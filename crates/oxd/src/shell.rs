//! Shell commands of workflow runs: checks and shell steps, each run with
//! `/bin/sh -c` in its own process group.

use std::collections::BTreeMap;
use std::future::Future;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use crate::worktree::LOCAL_ENV_VARS;

/// How much of a command's output is kept: the end of it.
pub const OUTPUT_LIMIT: usize = 64 * 1024;

/// How long a stopped command has to exit after `SIGTERM` before its group
/// gets `SIGKILL`.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// The texts a command receives: `OX_RUN_INPUT` and `OX_STEP_INPUT`.
pub struct Inputs<'a> {
    /// The run's input.
    pub run: &'a str,
    /// The step's input: the previous step's output, or the run's input for
    /// the first step.
    pub step: &'a str,
}

/// A command that ran to its end.
pub struct Finished {
    /// How the shell exited. Unset when the command could not start.
    pub exit: Option<ExitStatus>,
    /// The whole standard output.
    pub stdout: String,
    /// The last [`OUTPUT_LIMIT`] bytes of standard output and standard error,
    /// interleaved as they arrived, or the error of a command that could not
    /// start.
    pub output: String,
    /// Whether output before the kept end was dropped.
    pub truncated: bool,
}

impl Finished {
    pub fn succeeded(&self) -> bool {
        self.exit.is_some_and(|exit| exit.success())
    }

    pub fn exit_code(&self) -> Option<i32> {
        self.exit.and_then(|exit| exit.code())
    }

    /// How the command ended: `exited with status <n>`, `was killed by
    /// signal <n>`, or `could not start`.
    pub fn result(&self) -> String {
        match self.exit {
            None => "could not start".into(),
            Some(exit) => match (exit.code(), exit.signal()) {
                (Some(code), _) => format!("exited with status {code}"),
                (None, Some(signal)) => format!("was killed by signal {signal}"),
                (None, None) => format!("ended with {exit}"),
            },
        }
    }
}

/// A command that `stop` ended, with what it wrote until then.
pub struct Stopped {
    pub output: String,
    pub truncated: bool,
}

/// Runs `command` with `/bin/sh -c` in `cwd`, with stdin null, `env` over the
/// daemon's environment without the variables that point git at one
/// repository, and `inputs`, in a process group of its own.
///
/// The command is done when its shell exits: the rest of its group then gets
/// `SIGKILL`, and its output is read to the end. When `stop` resolves first,
/// the group gets `SIGTERM`, and `SIGKILL` if the shell has not exited
/// [`STOP_GRACE`] later. Dropping the future sends the group `SIGKILL`.
pub async fn run(
    command: &str,
    cwd: &Path,
    env: &BTreeMap<String, String>,
    inputs: Inputs<'_>,
    stop: impl Future<Output = ()>,
) -> Result<Finished, Stopped> {
    let mut shell = Command::new("/bin/sh");
    shell
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .envs(env)
        .env("OX_RUN_INPUT", inputs.run)
        .env("OX_STEP_INPUT", inputs.step)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    for var in LOCAL_ENV_VARS {
        shell.env_remove(var);
    }
    let mut child = match shell.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ok(Finished {
                exit: None,
                stdout: String::new(),
                output: error.to_string(),
                truncated: false,
            });
        }
    };
    let mut group = Group(
        child
            .id()
            .and_then(|id| libc::pid_t::try_from(id).ok())
            .expect("a child that was not waited for has a process ID"),
    );
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    let mut output = Output::default();
    let mut reading = Box::pin(output.read(stdout, stderr));
    let exited = {
        let finished = async {
            let (status, ()) = tokio::join!(
                async {
                    let status = child.wait().await;
                    group.kill(libc::SIGKILL);
                    status
                },
                &mut reading
            );
            status
        };
        tokio::pin!(finished);
        tokio::select! {
            status = &mut finished => Some(status),
            () = stop => None,
        }
    };
    let Some(status) = exited else {
        group.kill(libc::SIGTERM);
        if tokio::time::timeout(STOP_GRACE, child.wait())
            .await
            .is_err()
        {
            group.kill(libc::SIGKILL);
            let _ = child.wait().await;
        }
        group.kill(libc::SIGKILL);
        reading.await;
        group.disarm();
        let (output, truncated) = output.tail();
        return Err(Stopped { output, truncated });
    };
    drop(reading);
    group.disarm();
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let (output, truncated) = output.tail();
    Ok(match status {
        Ok(status) => Finished {
            exit: Some(status),
            stdout,
            output,
            truncated,
        },
        Err(error) => Finished {
            exit: None,
            stdout,
            output: format!("waiting for the command failed: {error}"),
            truncated: false,
        },
    })
}

/// A command's process group, which gets `SIGKILL` when the guard is dropped
/// before it is disarmed.
struct Group(libc::pid_t);

impl Group {
    fn kill(&self, signal: libc::c_int) {
        if self.0 > 0 {
            // SAFETY: sends a signal to the process group of a command that
            // `run` started and whose group it has not yet disarmed.
            unsafe { libc::killpg(self.0, signal) };
        }
    }

    /// Stops the guard once the group is known to be gone.
    fn disarm(&mut self) {
        self.0 = 0;
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill(libc::SIGKILL);
    }
}

/// What a command has written.
#[derive(Default)]
struct Output {
    stdout: Vec<u8>,
    /// Standard output and standard error as they arrived, cut from the front
    /// once it is twice the limit.
    both: Vec<u8>,
    dropped: bool,
}

impl Output {
    /// Reads both streams to their end.
    async fn read(
        &mut self,
        mut stdout: impl AsyncRead + Unpin,
        mut stderr: impl AsyncRead + Unpin,
    ) {
        let mut out_buf = [0; 8192];
        let mut err_buf = [0; 8192];
        let (mut out_open, mut err_open) = (true, true);
        while out_open || err_open {
            tokio::select! {
                read = stdout.read(&mut out_buf), if out_open => match read {
                    Ok(n) if n > 0 => {
                        self.stdout.extend_from_slice(&out_buf[..n]);
                        self.push(&out_buf[..n]);
                    }
                    _ => out_open = false,
                },
                read = stderr.read(&mut err_buf), if err_open => match read {
                    Ok(n) if n > 0 => self.push(&err_buf[..n]),
                    _ => err_open = false,
                },
            }
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.both.extend_from_slice(bytes);
        if self.both.len() > 2 * OUTPUT_LIMIT {
            let cut = self.both.len() - OUTPUT_LIMIT;
            self.both.drain(..cut);
            self.dropped = true;
        }
    }

    /// The last [`OUTPUT_LIMIT`] bytes, starting at a character boundary, and
    /// whether anything before them was dropped.
    fn tail(&self) -> (String, bool) {
        let mut start = self.both.len().saturating_sub(OUTPUT_LIMIT);
        let truncated = self.dropped || start > 0;
        // A UTF-8 continuation byte is 0b10xx_xxxx.
        while start < self.both.len() && start > 0 && self.both[start] & 0xC0 == 0x80 {
            start += 1;
        }
        (
            String::from_utf8_lossy(&self.both[start..]).into_owned(),
            truncated,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    const INPUTS: Inputs<'static> = Inputs {
        run: "the run",
        step: "the step",
    };

    async fn finish(command: &str) -> Finished {
        let dir = tempfile::tempdir().unwrap();
        run(
            command,
            dir.path(),
            &BTreeMap::new(),
            INPUTS,
            std::future::pending(),
        )
        .await
        .ok()
        .expect("the command was not stopped")
    }

    #[tokio::test]
    async fn stdout_is_kept_apart_and_interleaved_with_stderr() {
        let finished =
            finish("printf a; sleep 0.1; printf b >&2; sleep 0.1; printf c; exit 3").await;
        assert_eq!(finished.stdout, "ac");
        assert_eq!(finished.output, "abc");
        assert!(!finished.truncated);
        assert_eq!(finished.exit_code(), Some(3));
        assert_eq!(finished.result(), "exited with status 3");
        assert!(!finished.succeeded());
    }

    #[tokio::test]
    async fn long_output_keeps_its_tail() {
        let finished =
            finish("printf start; head -c 200000 /dev/zero | tr '\\0' x; printf end").await;
        assert!(finished.succeeded());
        assert!(finished.truncated);
        assert_eq!(finished.output.len(), OUTPUT_LIMIT);
        assert!(finished.output.ends_with("xxend"));
        assert_eq!(finished.stdout.len(), 200_008);
        assert!(finished.stdout.starts_with("start"));
    }

    #[tokio::test]
    async fn the_inputs_reach_the_command() {
        let finished = finish(r#"printf '%s|%s' "$OX_RUN_INPUT" "$OX_STEP_INPUT""#).await;
        assert_eq!(finished.stdout, "the run|the step");
    }

    #[tokio::test]
    async fn a_background_process_holding_stdout_does_not_hold_the_command() {
        let started = Instant::now();
        let finished = finish("sleep 30 & echo done").await;
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(finished.stdout, "done\n");
    }

    #[tokio::test]
    async fn a_command_that_cannot_start_fails_with_the_error() {
        let finished = run(
            "true",
            Path::new("/nonexistent/directory"),
            &BTreeMap::new(),
            INPUTS,
            std::future::pending(),
        )
        .await
        .ok()
        .unwrap();
        assert_eq!(finished.exit, None);
        assert_eq!(finished.result(), "could not start");
        assert!(!finished.output.is_empty());
    }

    #[tokio::test]
    async fn stop_kills_a_command_that_ignores_sigterm() {
        let dir = tempfile::tempdir().unwrap();
        let started = Instant::now();
        let stopped = run(
            "trap '' TERM; echo ready; sleep 30",
            dir.path(),
            &BTreeMap::new(),
            INPUTS,
            tokio::time::sleep(Duration::from_millis(200)),
        )
        .await
        .err()
        .expect("the command was stopped");
        let elapsed = started.elapsed();
        assert!(elapsed >= STOP_GRACE, "{elapsed:?}");
        assert!(elapsed < STOP_GRACE + Duration::from_secs(5), "{elapsed:?}");
        assert_eq!(stopped.output, "ready\n");
    }
}
