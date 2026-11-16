//! The pipes of a bridge's native process.

use std::{io, os::fd::AsRawFd, process::ExitStatus};

use serde_json::Value;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStderr, ChildStdin, ChildStdout},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

/// Writes each message to the process's stdin as a line, until the sender
/// drops or the process closes its stdin.
pub async fn write_input(mut stdin: ChildStdin, mut messages: mpsc::UnboundedReceiver<Value>) {
    while let Some(message) = messages.recv().await {
        let mut line = message.to_string().into_bytes();
        line.push(b'\n');
        if stdin.write_all(&line).await.is_err() {
            return;
        }
    }
}

/// Passes each line of the process's stdout to `translate` until the
/// process exits. Returns its exit status and what it wrote to stderr.
pub async fn read_output(
    child: &mut Child,
    mut translate: impl FnMut(&str),
) -> io::Result<(ExitStatus, Vec<u8>)> {
    let mut stderr = Stderr::collect(child.stderr.take().expect("piped stderr"));
    let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
    let mut translate_lines = |bytes: &[u8]| {
        for line in bytes.split(|&byte| byte == b'\n') {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if !line.is_empty() {
                translate(&String::from_utf8_lossy(line));
            }
        }
    };
    // A line being read; `read_until` keeps a partial line here when the
    // other branch wins.
    let mut line = Vec::new();
    let status = loop {
        tokio::select! {
            biased;
            read = stdout.read_until(b'\n', &mut line) => match read {
                Ok(n) if n > 0 => {
                    translate_lines(&line);
                    line.clear();
                }
                _ => break child.wait().await,
            },
            status = child.wait() => {
                // Everything the process wrote is in the pipe; a process it
                // started may hold the pipe open.
                drain(&stdout, &mut line);
                translate_lines(&line);
                break status;
            }
        }
    }?;
    Ok((status, stderr.finish().await))
}

/// Appends what the process's stdout holds once the process has exited: the
/// reader's buffer, then the pipe's contents, read from the descriptor
/// directly because the runtime may not yet have seen the pipe become
/// readable.
fn drain(stdout: &BufReader<ChildStdout>, bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(stdout.buffer());
    read_buffered(stdout.get_ref(), bytes);
}

/// Reads the process's stderr while it runs, so a full pipe never blocks it.
struct Stderr {
    finish: Option<oneshot::Sender<()>>,
    task: JoinHandle<Vec<u8>>,
}

impl Stderr {
    fn collect(stderr: ChildStderr) -> Self {
        let (finish, finished) = oneshot::channel();
        Self {
            finish: Some(finish),
            task: tokio::spawn(read_stderr(stderr, finished)),
        }
    }

    /// Everything the process wrote to stderr. Call it once the process has
    /// exited: what it wrote is then in the pipe, and a process it started
    /// that still holds the pipe open does not delay the result.
    async fn finish(&mut self) -> Vec<u8> {
        if let Some(finish) = self.finish.take() {
            let _ = finish.send(());
        }
        (&mut self.task).await.unwrap_or_default()
    }
}

impl Drop for Stderr {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_stderr(mut stderr: ChildStderr, mut finished: oneshot::Receiver<()>) -> Vec<u8> {
    let mut buffer = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        tokio::select! {
            read = stderr.read(&mut chunk) => match read {
                Ok(n) if n > 0 => buffer.extend_from_slice(&chunk[..n]),
                _ => return buffer,
            },
            _ = &mut finished => {
                read_buffered(&stderr, &mut buffer);
                return buffer;
            }
        }
    }
}

/// Reads what the pipe holds now. Tokio opens child pipes non-blocking, so
/// `read` fails with `EAGAIN` instead of waiting once the pipe is empty.
fn read_buffered(pipe: &impl AsRawFd, buffer: &mut Vec<u8>) {
    let mut chunk = [0u8; 8192];
    loop {
        // SAFETY: reads at most `chunk.len()` bytes into `chunk` from a
        // descriptor `pipe` owns and keeps open for the call.
        let n = unsafe { libc::read(pipe.as_raw_fd(), chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            return;
        }
        buffer.extend_from_slice(&chunk[..n.cast_unsigned()]);
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::*;

    /// The process's last lines reach the pipe before it exits, and the
    /// bridge learns of the exit before the runtime sees the pipe become
    /// readable. Draining reads them all the same.
    #[tokio::test(flavor = "current_thread")]
    async fn drain_reads_what_an_exited_process_left_in_the_pipe() {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "printf 'first\\nsecond\\npartial'"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        assert!(child.wait().unwrap().success());
        // The runtime registers the pipe here and, with this task not
        // yielding, does not learn that it is readable.
        let stdout = BufReader::new(ChildStdout::from_std(stdout).unwrap());

        let mut bytes = Vec::new();
        drain(&stdout, &mut bytes);
        assert_eq!(bytes, b"first\nsecond\npartial");
    }
}
