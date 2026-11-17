//! Daemon client for the desktop end-to-end and live smoke tests, and for
//! running workflows.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use ox_protocol::schema::{ContentBlock, SessionConfigKind, TextContent};
use ox_protocol::{
    Checkout, Client, Comparison, ConfigValue, End, EventKind, ListingChange, RunInfo, RunState,
    SessionState,
};

#[derive(Parser)]
#[command(
    name = "oxctl",
    about = "Daemon client for Ox integration tests and workflow runs"
)]
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
        /// branch named for the worktree
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
    /// Send a text prompt now: it starts a turn on an idle session and
    /// steers a running one
    Prompt { session: String, text: String },
    /// Add, list, edit, remove, and send a session's queued prompts
    Queue {
        #[command(subcommand)]
        command: QueueCommand,
    },
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
    /// Start, follow, cancel, and remove workflow runs
    Run {
        #[command(subcommand)]
        command: RunCommand,
    },
}

#[derive(Subcommand)]
enum RunCommand {
    /// Run a workflow in a new worktree of a workspace and print the run's id
    Start {
        /// The workspace's id, as `workspace ls` shows it
        #[arg(long)]
        workspace: String,
        /// The workflow's file name in the workflows directory, without
        /// `.yaml`
        workflow: String,
        /// The text the first step receives
        input: String,
    },
    /// List runs as their id, state, and workflow name
    Ls,
    /// Wait until a run ends, print it as JSON, and fail unless it succeeded
    Wait { run: String },
    /// Cancel a running run
    Cancel { run: String },
    /// Remove a run that has ended, with its child sessions, its worktree,
    /// and the branch Ox created for it, unless the branch has commits on no
    /// other branch
    Rm {
        run: String,
        /// Remove the worktree even if that loses uncommitted changes or
        /// commits
        #[arg(long)]
        discard: bool,
    },
}

#[derive(Subcommand)]
enum QueueCommand {
    /// Queue a text prompt and print its id
    Add { session: String, text: String },
    /// List queued prompts as their id and text
    Ls { session: String },
    /// Replace a queued prompt's text
    Edit {
        session: String,
        prompt: String,
        text: String,
    },
    /// Remove a queued prompt
    Rm { session: String, prompt: String },
    /// Take a prompt out of the queue and send it now
    Send { session: String, prompt: String },
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

fn text_prompt(text: String) -> Vec<ContentBlock> {
    vec![ContentBlock::Text(TextContent::new(text))]
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
            for change in listing(&client).await? {
                let ListingChange::Session { session } = change else {
                    continue;
                };
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
                for change in listing(&client).await? {
                    if let ListingChange::Workspace { workspace } = change {
                        println!("{}\t{}", workspace.id, workspace.path.display());
                    }
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
                Checkout::Worktree { compare: None }
            } else {
                Checkout::Workspace
            };
            let session = client
                .new_session(&workspace, command, utf8_env(), checkout, Vec::new(), None)
                .await?;
            println!("{}", started(&client, &session.id).await?);
        }
        Command::Load { session } => {
            client.load_session(&session, utf8_env()).await?;
            println!("{}", loaded(&client, &session).await?);
        }
        Command::Prompt { session, text } => client.prompt(&session, text_prompt(text)).await?,
        Command::Queue { command } => match command {
            QueueCommand::Add { session, text } => {
                println!("{}", client.queue_add(&session, text_prompt(text)).await?);
            }
            QueueCommand::Ls { session } => {
                for change in listing(&client).await? {
                    if let ListingChange::Session { session: info } = change
                        && info.id == session
                    {
                        for queued in info.queue {
                            let text: Vec<&str> = queued
                                .prompt
                                .iter()
                                .filter_map(|block| match block {
                                    ContentBlock::Text(text) => Some(text.text.as_str()),
                                    _ => None,
                                })
                                .collect();
                            println!("{}\t{}", queued.id, text.join(" ").replace('\n', " "));
                        }
                    }
                }
            }
            QueueCommand::Edit {
                session,
                prompt,
                text,
            } => {
                client
                    .queue_edit(&session, &prompt, text_prompt(text))
                    .await?;
            }
            QueueCommand::Rm { session, prompt } => client.queue_remove(&session, &prompt).await?,
            QueueCommand::Send { session, prompt } => client.queue_send(&session, &prompt).await?,
        },
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
            let worktree_id = listing(&client)
                .await?
                .into_iter()
                .find_map(|change| match change {
                    ListingChange::Session { session: info } if info.id == session => Some(info),
                    _ => None,
                })
                .ok_or_else(|| format!("no session {session}"))?
                .worktree_id;
            let Some(worktree_id) = worktree_id else {
                return Ok(());
            };
            let info = client.refresh(&worktree_id).await?;
            match (info.compare, info.comparison) {
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
        Command::Run { command } => match command {
            RunCommand::Start {
                workspace,
                workflow,
                input,
            } => {
                let run = client
                    .start_run(&workspace, &workflow, &input, utf8_env())
                    .await?;
                println!("{}", run.id);
            }
            RunCommand::Ls => {
                for change in listing(&client).await? {
                    if let ListingChange::Run { run } = change {
                        let state = serde_json::to_value(run.state)?;
                        let state = state.as_str().unwrap_or_default();
                        println!("{}\t{state}\t{}", run.id, run.name);
                    }
                }
            }
            RunCommand::Wait { run } => {
                let run = ended(&client, &run).await?;
                println!("{}", serde_json::to_string(&run)?);
                if run.state != RunState::Succeeded {
                    let state = serde_json::to_value(run.state)?;
                    let state = state.as_str().unwrap_or_default();
                    return Err(match &run.error {
                        Some(error) => format!("the run {state}: {error}"),
                        None => format!("the run {state}"),
                    }
                    .into());
                }
            }
            RunCommand::Cancel { run } => client.cancel_run(&run).await?,
            RunCommand::Rm { run, discard } => {
                client.delete_run(&run, discard, utf8_env()).await?;
            }
        },
    }
    Ok(())
}

/// Every workspace, worktree, session and run, as a watch's first changes
/// list them.
async fn listing(client: &Client) -> Result<Vec<ListingChange>, Box<dyn Error>> {
    let mut watch = client.watch().await?;
    Ok(watch.next().await.map_err(|_| ox_protocol::Error::Closed)?)
}

/// Waits until a run is not running and returns it.
async fn ended(client: &Client, run_id: &str) -> Result<RunInfo, Box<dyn Error>> {
    let mut watch = client.watch().await?;
    // The watch's first changes list every run.
    let mut listed = false;
    loop {
        let changes = watch.next().await.map_err(|_| ox_protocol::Error::Closed)?;
        let mut current = None;
        for change in changes {
            match change {
                ListingChange::Run { run } if run.id == run_id => current = Some(run),
                ListingChange::RunRemoved { run_id: id } if id == run_id => {
                    return Err(format!("the run {run_id} was removed").into());
                }
                _ => {}
            }
        }
        if !listed && current.is_none() {
            return Err(format!("no run {run_id}").into());
        }
        listed = true;
        if let Some(run) = current.filter(|run| run.state != RunState::Running) {
            return Ok(run);
        }
    }
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
    let closed = || "the session closed while its agent was starting".into();
    let mut watch = client.watch().await?;
    // The watch's first changes list every session.
    let mut listed = false;
    loop {
        let changes = watch.next().await.map_err(|_| ox_protocol::Error::Closed)?;
        let mut state = None;
        for change in changes {
            match change {
                ListingChange::Session { session } if session.id == session_id => {
                    state = Some(session.state);
                }
                ListingChange::SessionRemoved { session_id: id } if id == session_id => {
                    return Err(closed());
                }
                _ => {}
            }
        }
        if !listed && state.is_none() {
            return Err(closed());
        }
        listed = true;
        match state {
            None | Some(SessionState::Starting) => {}
            Some(SessionState::Exited) => {
                return Err(format!(
                    "the agent did not start; `oxctl attach {session_id}` shows why"
                )
                .into());
            }
            Some(_) => return Ok(session_id.to_owned()),
        }
    }
}

/// The environment variables whose names and values are UTF-8.
fn utf8_env() -> BTreeMap<String, String> {
    std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}
