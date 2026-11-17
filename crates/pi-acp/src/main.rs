mod bridge;
mod commands;
mod content;
mod dialogs;
mod history;
mod options;
mod pi;
mod tools;
mod updates;

use std::{env, process};

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
    acp_bridge::serve(Agent.v2().name("pi-acp"), bridge::Bridge::new()).await
}
