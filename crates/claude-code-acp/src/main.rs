mod background;
mod bridge;
mod commands;
mod content;
mod history;
mod questions;
mod runner;
mod updates;

use std::{env, path::PathBuf};

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
    let bridge = bridge::Bridge::new(&claude_dir);
    acp_bridge::serve(Agent.v2().name("claude-code-acp"), bridge).await
}
