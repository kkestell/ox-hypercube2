mod background;
mod bridge;
mod commands;
mod content;
mod history;
mod questions;
mod runner;
mod sessions;
mod updates;

use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process,
};

use agent_client_protocol::{Agent, Result};

#[tokio::main]
async fn main() -> Result<()> {
    // Claude keeps its transcripts under `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    let claude_dir = env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".claude")
        });
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    let history = || history::History::new(&claude_dir);
    let printed = match args.as_slice() {
        // `--describe <dir>` prints what a new session in `dir` offers.
        [flag, dir] if flag == "--describe" => bridge::describe(dir.as_ref()).await,
        // `--sessions <dir>` prints the interactive sessions started in `dir`.
        [flag, dir] if flag == "--sessions" => {
            directory(dir).map(|dir| sessions::sessions(&history(), &dir))
        }
        // `--transcript <dir> <id>` prints a session's title and timed updates.
        [flag, dir, id] if flag == "--transcript" => directory(dir)
            .and_then(|dir| sessions::transcript(&history(), &dir, &id.to_string_lossy())),
        _ => {
            let bridge = bridge::Bridge::new(&claude_dir);
            return acp_bridge::serve(Agent.v2().name("claude-code-acp"), bridge).await;
        }
    };
    match printed {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!("{error}");
            process::exit(1);
        }
    }
    Ok(())
}

fn directory(dir: &OsString) -> std::result::Result<PathBuf, String> {
    acp_bridge::cwd(Path::new(dir))
}
