//! Daemon client for the desktop end-to-end and live Claude smoke tests.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ox_protocol::schema::{
    ContentBlock, ElicitationAcceptAction, ElicitationAction, SessionConfigKind, TextContent,
};
use ox_protocol::{Checkout, Client, Comparison, ConfigValue, End, EventKind, SessionState};

#[derive(Parser)]
#[command(name = "oxctl", about = "Daemon client for Ox integration tests")]
struct Cli {
    /// oxd socket [default: $`OX_SOCKET`, else oxd.sock in $`XDG_DATA_HOME`/ox, else ~/.local/share/ox]
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List sessions
    Ls,
    /// Add, list, and remove workspaces
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// Start a session in a workspace and print its id
    New {
        /// The workspace's id, as `workspace ls` shows it
        #[arg(long)]
        workspace: String,
        /// Run in a new worktree of the workspace's repository, on a new
        /// branch named for the session
        #[arg(long)]
        worktree: bool,
        /// Agent command
        #[arg(last = true, required = true)]
        command: Vec<String>,
    },
    /// Restart the agent of an exited session, replaying its conversation,
    /// and print the session's id
    Load {
        /// The session's id, as `ls` shows it
        session: String,
    },
    /// Send a text prompt
    Prompt { session: String, text: String },
    /// Set a session's select option and print its current value
    Config {
        session: String,
        option: String,
        value: String,
    },
    /// Replay a session's events and follow live events as JSON lines
    Attach {
        session: String,
        /// Print only events after this seq
        #[arg(long, default_value_t = 0)]
        after: u64,
    },
    /// Answer a permission request
    Permit {
        session: String,
        request_id: String,
        option_id: String,
    },
    /// Accept an elicitation
    Answer {
        session: String,
        request_id: String,
        /// The form's content as a JSON object, such as '{"color": "blue"}'
        content: String,
    },
    /// Cancel the running turn
    Cancel { session: String },
    /// Stop a session's agent and delete the session, with its worktree
    /// and the branch Ox created for it, unless the branch has commits on no
    /// other branch
    Delete {
        session: String,
        /// Delete the worktree even if that loses uncommitted changes or
        /// commits
        #[arg(long)]
        discard: bool,
    },
    /// Stop a session's agent and remove its worktree, keeping the branch
    /// and the conversation
    Archive { session: String },
    /// Add an archived session's worktree again
    Unarchive { session: String },
    /// Refresh and print a worktree session's branch comparison
    Compare { session: String },
}

#[derive(Subcommand)]
enum WorkspaceCommand {
    /// Add a directory as a workspace and print its id
    Add { path: PathBuf },
    /// List workspaces
    Ls,
    /// Remove a workspace that has no sessions
    Rm { workspace: String },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("oxctl: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    let socket = cli.socket.unwrap_or_else(ox_protocol::default_socket_path);
    let client = Client::connect_or_start(&socket).await?;
    match cli.command {
        Command::Ls => {
            for session in client.list().await? {
                println!(
                    "{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    session.id,
                    session.workspace_id,
                    if session.archived {
                        "archived"
                    } else {
                        session.state.as_str()
                    },
                    session.cwd.display(),
                    session.command.join(" "),
                    session.title.as_deref().unwrap_or("-"),
                    session.agent_session_id.as_deref().unwrap_or("-"),
                );
            }
        }
        Command::Workspace { command } => match command {
            WorkspaceCommand::Add { path } => {
                let workspace = client.add_workspace(path.canonicalize()?).await?;
                println!("{}", workspace.id);
            }
            WorkspaceCommand::Ls => {
                for workspace in client.list_workspaces().await? {
                    println!("{}\t{}", workspace.id, workspace.path.display());
                }
            }
            WorkspaceCommand::Rm { workspace } => client.remove_workspace(&workspace).await?,
        },
        Command::New {
            workspace,
            worktree,
            command,
        } => {
            let checkout = if worktree {
                Checkout::Worktree {
                    branch: None,
                    compare: None,
                }
            } else {
                Checkout::Workspace
            };
            let session = client
                .new_session(&workspace, command, utf8_env(), checkout)
                .await?;
            println!("{}", started(&client, &session.id).await?);
        }
        Command::Load { session } => {
            client.load_session(&session, utf8_env()).await?;
            println!("{}", loaded(&client, &session).await?);
        }
        Command::Prompt { session, text } => {
            let prompt = vec![ContentBlock::Text(TextContent::new(text))];
            client.prompt(&session, prompt).await?;
        }
        Command::Config {
            session,
            option,
            value,
        } => {
            let options = client
                .set_config_option(&session, &option, ConfigValue::Id(value))
                .await?;
            let current = options
                .into_iter()
                .find(|candidate| *candidate.config_id.0 == *option);
            let Some(SessionConfigKind::Select(select)) = current.map(|config| config.kind) else {
                return Err(
                    format!("the agent did not return a select option for {option}").into(),
                );
            };
            println!("{}", select.current_value);
        }
        Command::Attach { session, after } => {
            let mut events = client.subscribe(&session, after).await?;
            loop {
                match events.next().await {
                    Ok(event) => println!("{}", serde_json::to_string(&event)?),
                    Err(End::SessionClosed) => break,
                    Err(End::Disconnected) => return Err(ox_protocol::Error::Closed.into()),
                }
            }
        }
        Command::Permit {
            session,
            request_id,
            option_id,
        } => client.permit(&session, &request_id, &option_id).await?,
        Command::Answer {
            session,
            request_id,
            content,
        } => {
            let fields = serde_json::from_str::<BTreeMap<_, _>>(&content)
                .map_err(|error| format!("the content is not a JSON object: {error}"))?;
            let action = ElicitationAction::Accept(ElicitationAcceptAction::new().content(fields));
            client
                .respond_elicitation(&session, &request_id, action)
                .await?;
        }
        Command::Cancel { session } => client.cancel(&session).await?,
        Command::Delete { session, discard } => {
            client.delete(&session, discard, utf8_env()).await?;
        }
        Command::Archive { session } => {
            client.archive(&session, false, utf8_env()).await?;
        }
        Command::Unarchive { session } => {
            client.unarchive(&session, utf8_env()).await?;
        }
        Command::Compare { session } => {
            let info = client.refresh(&session).await?;
            let reference = info.worktree.and_then(|worktree| worktree.compare);
            match (reference, info.comparison) {
                (
                    Some(reference),
                    Some(Comparison::Compared {
                        tip,
                        compare_tip,
                        ahead,
                        behind,
                    }),
                ) => println!("{reference}\t{tip}\t{compare_tip}\t{ahead}\t{behind}"),
                (_, Some(Comparison::Failed { error })) => return Err(error.into()),
                _ => {}
            }
        }
    }
    Ok(())
}

/// Waits for a session's agent to start and returns the session's id. A
/// session whose agent fails to start is deleted.
async fn started(client: &Client, session_id: &str) -> Result<String, Box<dyn Error>> {
    let mut events = client.subscribe(session_id, 0).await?;
    loop {
        match events.next().await {
            Ok(event) => match event.kind {
                EventKind::AgentStarted { .. } => return Ok(session_id.to_owned()),
                EventKind::AgentExited { error } => {
                    // The agent never ran, so the worktree has nothing of
                    // its own to lose.
                    client.delete(session_id, true, utf8_env()).await?;
                    let error = error.unwrap_or_else(|| "it exited".into());
                    return Err(format!("the session did not start: {error}").into());
                }
                _ => {}
            },
            Err(End::SessionClosed) => {
                return Err("the session closed while its agent was starting".into());
            }
            Err(End::Disconnected) => return Err(ox_protocol::Error::Closed.into()),
        }
    }
}

/// Waits for a loading session's agent to start and returns the session's id.
/// A session whose agent fails to start stays exited, and can be loaded again.
async fn loaded(client: &Client, session_id: &str) -> Result<String, Box<dyn Error>> {
    let mut watch = client.watch().await?;
    loop {
        let listing = watch.next().await.map_err(|_| ox_protocol::Error::Closed)?;
        let Some(session) = listing
            .sessions
            .iter()
            .find(|session| session.id == session_id)
        else {
            return Err("the session closed while its agent was starting".into());
        };
        match session.state {
            SessionState::Starting => {}
            SessionState::Exited => {
                return Err(format!(
                    "the agent did not start; `oxctl attach {session_id}` shows why"
                )
                .into());
            }
            _ => return Ok(session_id.to_owned()),
        }
    }
}

/// The environment variables whose names and values are UTF-8.
fn utf8_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}
