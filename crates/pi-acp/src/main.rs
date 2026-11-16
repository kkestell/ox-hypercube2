mod bridge;
mod commands;
mod content;
mod dialogs;
mod history;
mod options;
mod pi;
mod tools;
mod updates;

use agent_client_protocol::{Agent, Result};

#[tokio::main]
async fn main() -> Result<()> {
    acp_bridge::serve(Agent.v2().name("pi-acp"), bridge::Bridge::new()).await
}
