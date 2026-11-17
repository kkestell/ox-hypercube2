use std::io;
use std::path::PathBuf;

use agent_client_protocol::v2::schema::{McpServer, McpServerStdio};

/// The `ox` MCP server that oxd gives each agent: `ox-mcp`, run by the
/// agent with the daemon's socket and the session's ID.
pub struct OxServer {
    /// `ox-mcp` beside the daemon's executable.
    command: PathBuf,
    /// The daemon's socket, as an absolute path.
    socket: PathBuf,
}

impl OxServer {
    /// The server for a daemon listening on `socket`, an absolute path.
    pub fn new(socket: PathBuf) -> io::Result<Self> {
        Ok(Self {
            command: std::env::current_exe()?.with_file_name("ox-mcp"),
            socket,
        })
    }

    /// The `ox` server for the session `session_id`, with the tools of a
    /// run's child session when `child` is set.
    pub fn for_session(&self, session_id: &str, child: bool) -> McpServer {
        let mut args = vec![
            "--socket".into(),
            self.socket.display().to_string(),
            "--session".into(),
            session_id.into(),
        ];
        if child {
            args.push("--child".into());
        }
        McpServer::Stdio(McpServerStdio::new("ox", self.command.clone()).args(args))
    }
}
