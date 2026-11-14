//! ox-mcp: the `ox` MCP server that oxd gives each agent it starts.
//!
//! Usage: `ox-mcp --socket PATH --session ID`
//!
//! Serves MCP on stdio until stdin closes. Each tool call connects to the
//! daemon listening on `PATH`, calls a daemon method for the oxd session
//! `ID`, and disconnects, so the server never keeps the daemon from exiting
//! idle.
//!
//! Tools:
//!
//! - `set_title { title }`: sets the session's title (`session.set_title`)

use std::path::PathBuf;
use std::process::ExitCode;

use ox_protocol::Client;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::transport::stdio;
use rmcp::{ServiceExt, tool, tool_router};

struct Args {
    socket: PathBuf,
    session: String,
}

fn parse_args() -> Result<Args, String> {
    let mut socket = None;
    let mut session = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("{} needs a value", arg.display()))?;
        match arg.to_str() {
            Some("--socket") => socket = Some(PathBuf::from(value)),
            Some("--session") => {
                session = Some(
                    value
                        .into_string()
                        .map_err(|value| format!("invalid --session: {}", value.display()))?,
                );
            }
            _ => return Err(format!("unexpected argument: {}", arg.display())),
        }
    }
    Ok(Args {
        socket: socket.ok_or("--socket is required")?,
        session: session.ok_or("--session is required")?,
    })
}

/// The `ox` server of one oxd session.
struct Ox {
    socket: PathBuf,
    session: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct SetTitle {
    /// The new title.
    title: String,
}

#[tool_router(server_handler)]
impl Ox {
    #[tool(
        description = "Set the title of this session in Ox, shown in the session list. Use a short phrase that names the task."
    )]
    async fn set_title(
        &self,
        Parameters(SetTitle { title }): Parameters<SetTitle>,
    ) -> Result<String, String> {
        let client = Client::connect(&self.socket)
            .await
            .map_err(|error| error.to_string())?;
        client
            .set_title(&self.session, &title)
            .await
            .map_err(|error| error.to_string())?;
        Ok("Title set.".into())
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(message) => {
            eprintln!("ox-mcp: {message}");
            return ExitCode::from(2);
        }
    };
    let server = Ox {
        socket: args.socket,
        session: args.session,
    };
    let result = match server.serve(stdio()).await {
        Ok(service) => service.waiting().await.map(drop).map_err(|e| e.to_string()),
        Err(error) => Err(error.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ox-mcp: {error}");
            ExitCode::FAILURE
        }
    }
}
