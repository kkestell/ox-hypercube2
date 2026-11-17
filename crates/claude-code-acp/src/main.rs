mod background;
mod bridge;
mod commands;
mod content;
mod history;
mod questions;
mod runner;
mod updates;

use std::{env, path::PathBuf, process};

use agent_client_protocol::{Agent, Result};

#[tokio::main]
async fn main() -> Result<()> {
    // `--describe <dir>` prints what a new session in `dir` offers.
    let args: Vec<_> = env::args_os().skip(1).collect();
    if let [flag, dir] = args.as_slice()
        && flag == "--describe"
    {
        match bridge::describe(dir.as_ref()).await {
            Ok(description) => println!("{description}"),
            Err(error) => {
                eprintln!("{error}");
                process::exit(1);
            }
        }
        return Ok(());
    }
    // Claude keeps its transcripts under `$CLAUDE_CONFIG_DIR`, else `~/.claude`.
    let claude_dir = env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".claude")
        });
    let bridge = bridge::Bridge::new(&claude_dir);
    acp_bridge::serve(Agent.v2().name("claude-code-acp"), bridge).await
}
