use std::collections::BTreeMap;
use std::path::PathBuf;

use agent_client_protocol::v2::schema::{
    AvailableCommand, ContentBlock, ElicitationAction, ElicitationSchema, PermissionOption,
    PromptCapabilities, RequestPermissionOutcome, RequestPermissionSubject, SessionConfigOption,
    SessionUpdate, StopReason, ToolCallId,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version of the client–daemon protocol, exchanged in `hello`.
pub const PROTOCOL_VERSION: u32 = 1;

/// A client request: `{"id": n, "method": "...", "params": {...}}`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub method: Method,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "method", content = "params")]
pub enum Method {
    #[serde(rename = "hello")]
    Hello { protocol_version: u32 },
    #[serde(rename = "session.list")]
    SessionList,
    /// Starts a watch: after the answer, the daemon sends every workspace,
    /// worktree, session, and run at once, then each one that is added,
    /// changes, or is removed. A workspace's info is worked out again each
    /// time a workspace or session is added or removed or a client connects
    /// or disconnects. The request's `id` names the watch in every message.
    #[serde(rename = "watch")]
    Watch,
    /// Adds a workspace. `path` must be an absolute path to a directory that
    /// no workspace has.
    #[serde(rename = "workspace.add")]
    WorkspaceAdd { path: PathBuf },
    /// Removes a workspace. Refused while any session, exited or not, or
    /// any run belongs to it.
    #[serde(rename = "workspace.remove")]
    WorkspaceRemove { workspace_id: String },
    #[serde(rename = "workspace.list")]
    WorkspaceList,
    /// The workspace's local and remote-tracking branches, for naming the
    /// branch of a new worktree, with the worktree that has each local
    /// branch checked out, and the branch the workspace's checkout is on.
    /// Refused outside a Git working tree. With `fetch`, the daemon first
    /// runs `git fetch --all`, shared with any fetch of the same repository
    /// already running and stopped after 60 seconds; a fetch that fails sets
    /// `fetch_error` and the lists are answered as they stand. Git runs with `env` over the daemon's
    /// environment, as for `session.new`.
    #[serde(rename = "workspace.branches")]
    WorkspaceBranches {
        workspace_id: String,
        #[serde(default)]
        fetch: bool,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Starts an agent in the workspace's directory or in a new worktree of
    /// its Git repository, as `checkout` says. The result is the session in
    /// state `starting`; its `agent_started` or `agent_exited` event
    /// follows. A worktree's branch is checked, and created unless it is a
    /// local branch already, before the answer, and the worktree is added
    /// during the session's startup. Git runs with `env` over the daemon's
    /// environment, as the agent does.
    #[serde(rename = "session.new")]
    SessionNew {
        workspace_id: String,
        command: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default)]
        checkout: Checkout,
    },
    /// Restarts the agent of an exited session that has an
    /// `agent_session_id`, from the session's command and directory, which
    /// must still exist. The
    /// result is the session in state `starting`. The agent replays the
    /// conversation before its `agent_started` event. A session with no
    /// stored events records the replay as `update` events; a session that has
    /// them, including one restored after an `oxd` restart, keeps them and
    /// drops the replay.
    #[serde(rename = "session.load")]
    SessionLoad {
        session_id: String,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    #[serde(rename = "session.prompt")]
    SessionPrompt {
        session_id: String,
        /// ACP content blocks. Blocks other than text and resource links
        /// need the matching prompt capability of the agent.
        prompt: Vec<ContentBlock>,
    },
    #[serde(rename = "session.cancel")]
    SessionCancel { session_id: String },
    /// Starts a subscription: after the answer, the daemon sends the
    /// session's events with `seq > after_seq`, from the daemon's database
    /// for a session restored after an `oxd` restart, then each later event. The
    /// request's `id` names the subscription in every event it delivers. A
    /// subscription ends with the session's `session_closed` event. An
    /// `after_seq` that no event will follow is refused.
    #[serde(rename = "session.subscribe")]
    SessionSubscribe {
        session_id: String,
        #[serde(default)]
        after_seq: u64,
    },
    /// Starts a directory watch: after the answer, the daemon sends the
    /// entries of the directory at `path` at once and again each time they
    /// differ from the last it sent. `path` must be absolute. Entries are
    /// sorted by name, and leave out version-control folders, OS clutter
    /// files, and names that are not UTF-8. A symlink has its target's kind;
    /// a broken one is a file. A directory that does not exist or cannot be
    /// read lists no entries, and the watch lists it once it can be read.
    /// The request's `id` names the watch in every listing.
    #[serde(rename = "directory.watch")]
    DirectoryWatch { path: PathBuf },
    /// Follows the UTF-8 text or read error of an absolute file path.
    #[serde(rename = "file.watch")]
    FileWatch { path: PathBuf },
    /// Ends the subscription, listing watch, directory watch, or file watch that request
    /// `subscription` started. Messages the daemon was sending it may arrive
    /// after the answer.
    #[serde(rename = "unsubscribe")]
    Unsubscribe { subscription: u64 },
    #[serde(rename = "permission.respond")]
    PermissionRespond {
        session_id: String,
        request_id: String,
        option_id: String,
    },
    /// Answers an open elicitation: `{"action": "accept", "content": {...}}`,
    /// `{"action": "decline"}`, or `{"action": "cancel"}` beside the IDs.
    /// Accepted content must match the request's schema: every key names a
    /// property, each value has its property's type and lies within its
    /// choices and limits, and every required property is present. `pattern`
    /// and `format` are not checked.
    #[serde(rename = "elicitation.respond")]
    ElicitationRespond {
        session_id: String,
        request_id: String,
        #[serde(flatten)]
        action: ElicitationAction,
    },
    #[serde(rename = "session.set_config_option")]
    SessionSetConfigOption {
        session_id: String,
        config_id: String,
        value: ConfigValue,
    },
    /// Stops the session's agent and deletes the session. A session in a
    /// worktree waits for its startup to finish, then has its worktree
    /// removed. The removal is refused with `UNSAVED_WORK`, leaving the
    /// session as it was, while the worktree has uncommitted changes,
    /// commits on no branch, or submodule commits on no remote branch,
    /// unless `discard` is set. A branch the daemon created is then deleted
    /// if all its commits are on another branch, tag or remote branch, and
    /// kept otherwise; a branch that existed before the session is always
    /// kept. Git runs with `env` over the daemon's environment, as for
    /// `session.new`. The result is [`Deleted`].
    #[serde(rename = "session.delete")]
    SessionDelete {
        session_id: String,
        #[serde(default)]
        discard: bool,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Stops the session's agent, removes its worktree, and marks the
    /// session archived, keeping its branch. The conversation stays readable
    /// by subscribing; `session.load` refuses an archived session. The
    /// archive is refused while the agent is starting. Worktree removal
    /// is refused with `UNSAVED_WORK`, as for
    /// `session.delete`, unless `discard` is set. Git runs with `env` over
    /// the daemon's environment. The result is the session's
    /// [`SessionInfo`].
    #[serde(rename = "session.archive")]
    SessionArchive {
        session_id: String,
        #[serde(default)]
        discard: bool,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Adds an archived session's worktree again, on its branch, and marks
    /// the session not archived. It is then an `exited` session, which
    /// `session.load` resumes. Git runs with `env` over the daemon's
    /// environment. The result is the session's [`SessionInfo`].
    #[serde(rename = "session.unarchive")]
    SessionUnarchive {
        session_id: String,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Sets local branch `branch` of the workspace's repository to commit
    /// `to`. Refused, changing nothing, when the branch is not at
    /// `expected`, or when `to` does not descend from `expected` unless
    /// `force` is set. When `source_branch` is supplied, that local branch
    /// must still be at `to` when the destination ref moves.
    /// When a checkout has the branch, its files and index
    /// move with it; the move is refused with `UNSAVED_WORK` while tracked
    /// files there have uncommitted changes or the move would overwrite
    /// ignored files. A move is refused while a session's agent there is
    /// starting, in a turn, or waiting for an answer. Git
    /// runs with `env` over the daemon's environment.
    #[serde(rename = "branch.move")]
    BranchMove {
        workspace_id: String,
        branch: String,
        to: String,
        expected: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_branch: Option<String>,
        #[serde(default)]
        force: bool,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Sets or clears a worktree's comparison ref and answers the worktree
    /// with its comparison recomputed. Refused for a ref that names no
    /// commit.
    #[serde(rename = "worktree.set_compare")]
    WorktreeSetCompare {
        worktree_id: String,
        compare: Option<String>,
    },
    /// Recomputes the worktree's comparison and answers the worktree.
    #[serde(rename = "worktree.refresh")]
    WorktreeRefresh { worktree_id: String },
    /// Sets the session's title, shown in the session list and saved with
    /// the session. Runs of whitespace become one space, and a title left
    /// empty is refused with `invalid_request`. Answers `null`.
    #[serde(rename = "session.set_title")]
    SessionSetTitle { session_id: String, title: String },
    /// Records a status update from the session's agent: what it is doing
    /// and what it will do next. The text is trimmed, and an empty one is
    /// refused with `invalid_request`. Refused with `invalid_state` unless
    /// the agent is running. The update is saved in the session's history
    /// as a `status_update` event. Answers `null`.
    #[serde(rename = "session.post_status_update")]
    SessionPostStatusUpdate { session_id: String, text: String },
    /// Starts a run of the workflow defined in `<workflow>.yaml` in the
    /// daemon's workflows directory, in a new worktree of the workspace's
    /// repository on a new branch `ox/<first 8 digits of the worktree ID>`
    /// that starts at `HEAD`. `input` goes to the first step. The result is
    /// the run, `running`. Refused, changing nothing, for a workflow that is
    /// not found (`NOT_FOUND`) or does not parse (`INVALID_REQUEST`), or a
    /// workspace outside a Git working tree. Git, the steps' agents, and
    /// their commands run with `env` over the daemon's environment.
    #[serde(rename = "run.start")]
    RunStart {
        workspace_id: String,
        workflow: String,
        input: String,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Cancels a running run: the active step's turn or command is
    /// cancelled, and the step and the run end `cancelled`. Later steps do
    /// not start. Refused for a run that is not `running`. Answers `null`.
    #[serde(rename = "run.cancel")]
    RunCancel { run_id: String },
    /// Removes a run that is not `running`: stops its child sessions'
    /// agents, removes its worktree, and deletes the run and its child
    /// sessions. The worktree's removal and its branch follow the rules of
    /// `session.delete`, including `discard`. Git runs with `env` over the
    /// daemon's environment. The result is [`Deleted`].
    #[serde(rename = "run.delete")]
    RunDelete {
        run_id: String,
        #[serde(default)]
        discard: bool,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Lists the definitions in the daemon's workflows directory: every
    /// `*.yaml` file whose name does not start with `.`, sorted by file
    /// name. The result is a list of [`WorkflowInfo`], empty when the
    /// directory does not exist.
    #[serde(rename = "workflow.list")]
    WorkflowList,
}

/// A coding-agent CLI that Ox drives through its ACP bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Harness {
    Claude,
    Pi,
}

impl Harness {
    pub const ALL: [Harness; 2] = [Harness::Claude, Harness::Pi];

    /// The name of the coding-agent CLI the harness's bridge drives.
    pub fn executable(self) -> &'static str {
        match self {
            Harness::Claude => "claude",
            Harness::Pi => "pi",
        }
    }

    /// The name of the harness's ACP bridge, which ships beside the app's
    /// and the daemon's executables.
    pub fn bridge(self) -> &'static str {
        match self {
            Harness::Claude => "claude-code-acp",
            Harness::Pi => "pi-acp",
        }
    }
}

/// Where a new session runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Checkout {
    /// The workspace's directory.
    #[default]
    Workspace,
    /// A new worktree of the workspace's repository, in the same
    /// subdirectory the workspace is of its checkout, on `branch`. With no
    /// `branch`, the worktree is on a new branch `ox/<first 8 digits of the
    /// worktree ID>` that starts at `HEAD`. With `compare`, a ref such as
    /// `main` that must name a commit, the worktree publishes how its branch
    /// compares with the ref.
    Worktree {
        #[serde(default)]
        branch: Option<WorktreeBranch>,
        #[serde(default)]
        compare: Option<String>,
    },
}

/// The branch a new worktree is on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorktreeBranch {
    /// A local branch no worktree has checked out, or a remote branch,
    /// named `<branch>` or `<remote>/<branch>`, that becomes a local branch
    /// tracking it. Refused if no such branch exists.
    Existing { name: String },
    /// A new branch that starts at `start`, `HEAD` when unset, with no
    /// upstream. Refused if a local branch, a remote branch's full name
    /// `<remote>/<branch>`, or a branch on any remote has the name.
    New {
        name: String,
        #[serde(default)]
        start: Option<String>,
    },
}

/// A worktree the daemon added for a session or a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeInfo {
    pub id: String,
    /// The worktree's top level. The session's `cwd` is this directory or a
    /// subdirectory of it.
    pub path: PathBuf,
    pub branch: String,
    /// The commit the session started at.
    pub base: String,
    /// Whether the daemon created the branch for the session.
    pub created_branch: bool,
    /// The ref the branch is compared with, as the client gave it.
    pub compare: Option<String>,
    /// How the branch compares with `compare`. Unset when there is no
    /// comparison ref. The daemon recomputes it when the session's agent
    /// ends a turn, after a branch move, a session delete or a fetch in the
    /// repository, and on `worktree.refresh`.
    pub comparison: Option<Comparison>,
}

/// How a worktree's branch compares with its comparison ref.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Comparison {
    /// `tip` is the branch's commit, `compare_tip` the ref's. `ahead`
    /// counts the branch's commits the ref lacks, `behind` the ref's
    /// commits the branch lacks.
    Compared {
        tip: String,
        compare_tip: String,
        ahead: u64,
        behind: u64,
    },
    /// Git could not compare them, such as when the ref or the branch is
    /// gone.
    Failed { error: String },
}

/// The result of `session.delete` and `run.delete`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deleted {
    /// The branch the daemon created for the worktree, kept
    /// because it has commits on no other branch, tag or remote branch.
    pub kept_branch: Option<String>,
}

/// A value for a session config option: the value id of a select option, or
/// a boolean.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    Boolean(bool),
    Id(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol_version: u32,
    pub daemon_version: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Starting,
    Idle,
    Running,
    AwaitingPermission,
    /// An elicitation is open and no permission request is.
    AwaitingInput,
    Exited,
}

impl SessionState {
    /// The state's name on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Idle => "idle",
            Self::Running => "running",
            Self::AwaitingPermission => "awaiting_permission",
            Self::AwaitingInput => "awaiting_input",
            Self::Exited => "exited",
        }
    }
}

/// A directory the user added, which sessions run in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub id: String,
    pub path: PathBuf,
    /// Whether the directory looks like it is in a Git working tree: it is
    /// not inside a `.git` directory, and it or a parent has a `.git`. Only
    /// then can a session run in a worktree of it. The daemon works this out
    /// without running git each time it lists the workspace.
    pub git_working_tree: bool,
}

/// A workspace's branches, each list most recently committed first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branches {
    /// The local branch the workspace's checkout is on, unset when its
    /// `HEAD` is detached or unborn.
    pub head: Option<String>,
    /// Local branches, such as `main`.
    pub local: Vec<LocalBranch>,
    /// Remote-tracking branch names with their remote, such as
    /// `origin/main`, without symbolic refs such as `origin/HEAD`.
    pub remote: Vec<String>,
    /// Why the fetch the request asked for failed, if it did.
    pub fetch_error: Option<String>,
}

/// A local branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalBranch {
    pub name: String,
    /// The top level of the worktree that has the branch checked out: the
    /// repository's own checkout, a session's worktree, or any other
    /// worktree of the repository.
    pub worktree: Option<PathBuf>,
}

/// The daemon's workspaces, in the order they were added, its worktrees, its
/// sessions, in the order they were created, and its runs, in the order they
/// were started.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    pub workspaces: Vec<WorkspaceInfo>,
    pub worktrees: Vec<WorktreeInfo>,
    pub sessions: Vec<SessionInfo>,
    pub runs: Vec<RunInfo>,
}

/// One change a watch delivers. A workspace, worktree, session or run that is
/// new comes after every one listed before it; one that changed keeps its
/// place. Each batch lists removals first, runs before sessions before
/// worktrees, then workspaces, worktrees, sessions and runs, so a session's
/// worktree arrives before the session, and a run's worktree and child
/// sessions before the run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ListingChange {
    /// A workspace that was added, or whose info changed.
    Workspace {
        workspace: WorkspaceInfo,
    },
    WorkspaceRemoved {
        workspace_id: String,
    },
    /// A worktree that was added, or whose info changed.
    Worktree {
        worktree: WorktreeInfo,
    },
    WorktreeRemoved {
        worktree_id: String,
    },
    /// A session that was added, or whose info changed.
    Session {
        session: Box<SessionInfo>,
    },
    SessionRemoved {
        session_id: String,
    },
    /// A run that was started, or whose info changed.
    Run {
        run: RunInfo,
    },
    RunRemoved {
        run_id: String,
    },
}

/// A definition in the daemon's workflows directory.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowInfo {
    /// The definition's file name without `.yaml`, which `run.start` takes.
    pub workflow: String,
    /// The definition's `name`, or `workflow` when the file does not parse.
    pub name: String,
    /// Why the file does not parse, or could not be read.
    pub error: Option<String>,
}

/// A run of a workflow: its steps, run one after another in a worktree the
/// run owns, each agent step in a child session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunInfo {
    pub id: String,
    pub workspace_id: String,
    /// The name of the definition's file, without `.yaml`, that the run was
    /// started with.
    pub workflow: String,
    /// The definition's `name`.
    pub name: String,
    /// The text the run was started with, which the first step receives.
    pub input: String,
    /// The worktree the run owns, which every step runs in.
    pub worktree_id: String,
    pub state: RunState,
    /// Why the run failed.
    pub error: Option<String>,
    /// Every step of the definition, in order.
    pub steps: Vec<StepInfo>,
    /// When the run was started, in milliseconds since the Unix epoch.
    pub created_at: u64,
}

/// A step of a run.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepInfo {
    pub name: String,
    pub state: StepState,
    /// When the step left `pending`, in milliseconds since the Unix epoch.
    pub started_at: Option<u64>,
    /// The child session the step runs in, once an agent step has started.
    /// Unset again when the session is deleted.
    pub session_id: Option<String>,
    /// The final reply of the step's last turn, or a shell step's standard
    /// output. The next step receives it once the step has succeeded. A step
    /// whose postcondition still fails after its repair turn keeps the repair
    /// turn's reply.
    pub output: Option<String>,
    /// The commands the step has started, in order.
    pub commands: Vec<CommandInfo>,
}

/// A command a step ran: a check or a shell step's command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInfo {
    pub kind: CommandKind,
    pub command: String,
    pub state: CommandState,
    /// Unset while the command runs, and when it was killed by a signal or
    /// could not start.
    pub exit_code: Option<i32>,
    /// The command's standard output and standard error, interleaved as they
    /// arrived, or the error of a command that could not start. Empty while
    /// the command runs. Once it ends, the last 64 KiB.
    pub output: String,
    /// Whether output before the last 64 KiB was dropped.
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    Precondition,
    /// A shell step's command.
    Shell,
    Postcondition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandState {
    Running,
    /// It exited with status 0.
    Succeeded,
    /// It exited with another status, was killed by a signal, or could not
    /// start.
    Failed,
    /// The run was cancelled while it ran.
    Cancelled,
    /// The daemon stopped while it ran.
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    /// The daemon stopped while the run was running. The run stays
    /// interrupted.
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    /// The daemon stopped while the step was running.
    Interrupted,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub workspace_id: String,
    pub command: Vec<String>,
    pub cwd: PathBuf,
    pub state: SessionState,
    /// The title the agent reported in a `session_info_update`.
    pub title: Option<String>,
    /// The agent's ID for the session, which `session.load` resumes. Unset
    /// until the session has started.
    pub agent_session_id: Option<String>,
    /// The content the agent accepts in prompts. Empty until the session has
    /// started.
    pub prompt_capabilities: PromptCapabilities,
    /// The agent's current config options, such as model and mode.
    pub config_options: Vec<SessionConfigOption>,
    /// The agent's complete current slash-command catalog.
    pub available_commands: Vec<AvailableCommand>,
    /// The ID of the worktree the session runs in, if it has one. Unset
    /// again when the worktree could not be added.
    pub worktree_id: Option<String>,
    /// Whether the session is archived: it is `exited`, its worktree is
    /// removed, and only its history can be read.
    pub archived: bool,
    /// When the session was created, in milliseconds since the Unix epoch.
    pub created_at: u64,
}

/// One entry in a session's history. `seq` starts at 1 and increases by one
/// per event within a session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub session_id: String,
    pub seq: u64,
    pub kind: EventKind,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
#[allow(
    clippy::large_enum_variant,
    reason = "most events are updates, so boxing them adds an allocation to nearly every event"
)]
pub enum EventKind {
    /// The agent answered `initialize` and `session/new`. It is the first
    /// event of a session that starts.
    AgentStarted {
        agent_session_id: String,
        prompt_capabilities: PromptCapabilities,
        config_options: Vec<SessionConfigOption>,
        available_commands: Vec<AvailableCommand>,
    },
    PromptStarted {
        prompt: Vec<ContentBlock>,
    },
    /// The agent reported work no prompt asked for, as Claude does when a
    /// background task it started reports back.
    TurnStarted,
    /// An ACP `session/update` notification, unchanged. When an agent answers
    /// `session.set_config_option` with options it has not reported in a
    /// notification, the daemon adds a `config_option_update` with them.
    Update(SessionUpdate),
    /// A `session/request_permission` request, unchanged. A tool call
    /// subject carries the tool call's details, which a client applies as it
    /// does a `tool_call_update`.
    PermissionRequested {
        request_id: String,
        title: String,
        description: Option<String>,
        subject: Option<RequestPermissionSubject>,
        options: Vec<PermissionOption>,
    },
    PermissionResolved {
        request_id: String,
        outcome: RequestPermissionOutcome,
    },
    /// A form-mode `elicitation/create` request, scoped to the session.
    /// Request IDs are shared with permission requests.
    ElicitationRequested {
        request_id: String,
        message: String,
        tool_call_id: Option<ToolCallId>,
        requested_schema: ElicitationSchema,
    },
    /// The elicitation was answered, or ended `cancel` because the agent
    /// withdrew it, exited, or the session was parked or closed.
    ElicitationResolved {
        request_id: String,
        action: ElicitationAction,
    },
    /// A turn finished. `stop_reason` is the one the agent reported with its
    /// `idle` state, if any. `error` is set when the agent rejected the
    /// prompt, exited, or the session was closed during the turn.
    TurnEnded {
        stop_reason: Option<StopReason>,
        error: Option<String>,
    },
    AgentExited {
        error: Option<String>,
    },
    /// The daemon stopped the agent because the session was idle, with no
    /// background work, and unviewed, or because the session was archived. The session is `exited`;
    /// `session.load` resumes it.
    AgentParked,
    /// The agent posted a status update through `session.post_status_update`.
    /// `posted_at` is in milliseconds since the Unix epoch.
    StatusUpdate {
        text: String,
        posted_at: u64,
    },
    /// The session was closed. It is the last event of a session.
    SessionClosed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: i32,
    pub message: String,
}

/// Error codes carried in [`ErrorBody::code`].
pub mod code {
    pub const INVALID_REQUEST: i32 = 1;
    pub const PROTOCOL_MISMATCH: i32 = 2;
    pub const NOT_FOUND: i32 = 3;
    pub const INVALID_STATE: i32 = 4;
    pub const AGENT: i32 = 5;
    /// The daemon could not read or write its database.
    pub const STORAGE: i32 = 6;
    /// Deleting the session would lose work in its worktree, which the
    /// message lists. Deleting it with `discard` set loses that work.
    pub const UNSAVED_WORK: i32 = 7;
    /// A git command failed. The message carries git's error output.
    pub const GIT: i32 = 8;
}

/// The current text of a file, or a readable failure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FileContent {
    Text { text: String },
    Error { message: String },
}

/// An entry of a directory a directory watch lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub kind: EntryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    File,
    Directory,
}

/// A daemon message: a response to request `id`, or an event or listing
/// delivered to the subscription or watch that request
/// `subscription` started.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerMessage {
    Error {
        id: u64,
        error: ErrorBody,
    },
    Result {
        id: u64,
        result: Value,
    },
    Event {
        subscription: u64,
        event: Box<Event>,
    },
    /// A watch's changes, in order. The first lists every workspace and
    /// session.
    List {
        subscription: u64,
        changes: Vec<ListingChange>,
    },
    Directory {
        subscription: u64,
        entries: Vec<DirEntry>,
    },
    File {
        subscription: u64,
        content: FileContent,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::v2::schema::ElicitationContentValue;
    use serde_json::json;

    #[test]
    fn request_round_trip() {
        let request: Request =
            serde_json::from_value(json!({"id": 3, "method": "session.list"})).unwrap();
        assert!(matches!(request.method, Method::SessionList));

        let request: Request = serde_json::from_value(json!({
            "id": 4,
            "method": "session.subscribe",
            "params": {"session_id": "s", "after_seq": 7}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::SessionSubscribe { ref session_id, after_seq: 7 } if session_id == "s"
        ));

        let request: Request = serde_json::from_value(json!({
            "id": 8,
            "method": "unsubscribe",
            "params": {"subscription": 4}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::Unsubscribe { subscription: 4 }
        ));
        let value = serde_json::to_value(Request {
            id: 9,
            method: Method::Watch,
        })
        .unwrap();
        assert_eq!(value, json!({"id": 9, "method": "watch"}));
        let request: Request = serde_json::from_value(value).unwrap();
        assert!(matches!(request.method, Method::Watch));

        let request: Request = serde_json::from_value(json!({
            "id": 12,
            "method": "directory.watch",
            "params": {"path": "/work/src"}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::DirectoryWatch { ref path } if path == std::path::Path::new("/work/src")
        ));

        let request: Request = serde_json::from_value(json!({
            "id": 10,
            "method": "workspace.add",
            "params": {"path": "/work"}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::WorkspaceAdd { ref path } if path == std::path::Path::new("/work")
        ));
        let request: Request = serde_json::from_value(json!({
            "id": 11,
            "method": "session.new",
            "params": {"workspace_id": "w", "command": ["agent"]}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::SessionNew { ref workspace_id, ref env, checkout: Checkout::Workspace, .. }
                if workspace_id == "w" && env.is_empty()
        ));
        let request: Request = serde_json::from_value(json!({
            "id": 14,
            "method": "session.new",
            "params": {"workspace_id": "w", "command": ["agent"], "checkout": {"type": "worktree"}}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::SessionNew {
                checkout: Checkout::Worktree {
                    branch: None,
                    compare: None
                },
                ..
            }
        ));
        for (branch, wire) in [
            (
                None,
                json!({"type": "worktree", "branch": null, "compare": null}),
            ),
            (
                Some(WorktreeBranch::Existing {
                    name: "feature".into(),
                }),
                json!({"type": "worktree", "branch": {"type": "existing", "name": "feature"}, "compare": null}),
            ),
            (
                Some(WorktreeBranch::New {
                    name: "topic".into(),
                    start: None,
                }),
                json!({"type": "worktree", "branch": {"type": "new", "name": "topic", "start": null}, "compare": null}),
            ),
            (
                Some(WorktreeBranch::New {
                    name: "topic".into(),
                    start: Some("main".into()),
                }),
                json!({"type": "worktree", "branch": {"type": "new", "name": "topic", "start": "main"}, "compare": null}),
            ),
        ] {
            let checkout = Checkout::Worktree {
                branch,
                compare: None,
            };
            let method = Method::SessionNew {
                workspace_id: "w".into(),
                command: vec!["agent".into()],
                env: BTreeMap::new(),
                checkout: checkout.clone(),
            };
            let value = serde_json::to_value(Request { id: 13, method }).unwrap();
            assert_eq!(value["params"]["checkout"], wire);
            let request: Request = serde_json::from_value(value).unwrap();
            assert!(matches!(
                request.method,
                Method::SessionNew { checkout: ref decoded, .. } if *decoded == checkout
            ));
        }
        assert_eq!(
            serde_json::from_value::<Checkout>(
                json!({"type": "worktree", "branch": {"type": "new", "name": "topic"}})
            )
            .unwrap(),
            Checkout::Worktree {
                branch: Some(WorktreeBranch::New {
                    name: "topic".into(),
                    start: None
                }),
                compare: None,
            }
        );
        let checkout = Checkout::Worktree {
            branch: None,
            compare: Some("main".into()),
        };
        let value = serde_json::to_value(&checkout).unwrap();
        assert_eq!(
            value,
            json!({"type": "worktree", "branch": null, "compare": "main"})
        );
        assert_eq!(serde_json::from_value::<Checkout>(value).unwrap(), checkout);
        let method = Method::BranchMove {
            workspace_id: "w".into(),
            branch: "main".into(),
            to: "abc".into(),
            expected: "def".into(),
            source_branch: Some("topic".into()),
            force: true,
            env: BTreeMap::from([("PATH".into(), "/bin".into())]),
        };
        let value = serde_json::to_value(Request { id: 21, method }).unwrap();
        assert_eq!(
            value,
            json!({
                "id": 21,
                "method": "branch.move",
                "params": {
                    "workspace_id": "w",
                    "branch": "main",
                    "to": "abc",
                    "expected": "def",
                    "source_branch": "topic",
                    "force": true,
                    "env": {"PATH": "/bin"}
                }
            })
        );
        let request: Request = serde_json::from_value(json!({
            "id": 22,
            "method": "branch.move",
            "params": {"workspace_id": "w", "branch": "main", "to": "abc", "expected": "def"}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::BranchMove { ref branch, ref to, ref expected, source_branch: None, force: false, ref env, .. }
                if branch == "main" && to == "abc" && expected == "def" && env.is_empty()
        ));
        for compare in [Some("origin/main".to_owned()), None] {
            let method = Method::WorktreeSetCompare {
                worktree_id: "t".into(),
                compare: compare.clone(),
            };
            let value = serde_json::to_value(Request { id: 23, method }).unwrap();
            assert_eq!(
                value,
                json!({
                    "id": 23,
                    "method": "worktree.set_compare",
                    "params": {"worktree_id": "t", "compare": compare}
                })
            );
            let request: Request = serde_json::from_value(value).unwrap();
            assert!(matches!(
                request.method,
                Method::WorktreeSetCompare { ref worktree_id, compare: ref decoded }
                    if worktree_id == "t" && *decoded == compare
            ));
        }
        let value = serde_json::to_value(Request {
            id: 24,
            method: Method::WorktreeRefresh {
                worktree_id: "t".into(),
            },
        })
        .unwrap();
        assert_eq!(
            value,
            json!({"id": 24, "method": "worktree.refresh", "params": {"worktree_id": "t"}})
        );
        let request: Request = serde_json::from_value(value).unwrap();
        assert!(matches!(
            request.method,
            Method::WorktreeRefresh { ref worktree_id } if worktree_id == "t"
        ));
        let request: Request = serde_json::from_value(json!({
            "id": 16,
            "method": "session.delete",
            "params": {"session_id": "s"}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::SessionDelete { ref session_id, discard: false, ref env }
                if session_id == "s" && env.is_empty()
        ));
        let request: Request = serde_json::from_value(json!({
            "id": 17,
            "method": "session.archive",
            "params": {"session_id": "s"}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::SessionArchive { ref session_id, discard: false, ref env }
                if session_id == "s" && env.is_empty()
        ));
        let request: Request = serde_json::from_value(json!({
            "id": 18,
            "method": "session.unarchive",
            "params": {"session_id": "s"}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::SessionUnarchive { ref session_id, ref env }
                if session_id == "s" && env.is_empty()
        ));
        let request: Request = serde_json::from_value(json!({
            "id": 15,
            "method": "workspace.branches",
            "params": {"workspace_id": "w"}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::WorkspaceBranches { ref workspace_id, fetch: false, ref env }
                if workspace_id == "w" && env.is_empty()
        ));
        let method = Method::WorkspaceBranches {
            workspace_id: "w".into(),
            fetch: true,
            env: BTreeMap::from([("PATH".into(), "/bin".into())]),
        };
        let value = serde_json::to_value(Request { id: 16, method }).unwrap();
        assert_eq!(
            value,
            json!({
                "id": 16,
                "method": "workspace.branches",
                "params": {"workspace_id": "w", "fetch": true, "env": {"PATH": "/bin"}}
            })
        );
        let branches = Branches {
            head: Some("main".into()),
            local: vec![
                LocalBranch {
                    name: "main".into(),
                    worktree: Some("/work".into()),
                },
                LocalBranch {
                    name: "topic".into(),
                    worktree: None,
                },
            ],
            remote: vec!["origin/main".into()],
            fetch_error: Some("no network".into()),
        };
        let value = serde_json::to_value(&branches).unwrap();
        assert_eq!(
            value,
            json!({
                "head": "main",
                "local": [
                    {"name": "main", "worktree": "/work"},
                    {"name": "topic", "worktree": null}
                ],
                "remote": ["origin/main"],
                "fetch_error": "no network"
            })
        );
        assert_eq!(serde_json::from_value::<Branches>(value).unwrap(), branches);
        let detached = Branches {
            head: None,
            local: Vec::new(),
            remote: Vec::new(),
            fetch_error: None,
        };
        let value = serde_json::to_value(&detached).unwrap();
        assert_eq!(
            value,
            json!({"head": null, "local": [], "remote": [], "fetch_error": null})
        );
        assert_eq!(serde_json::from_value::<Branches>(value).unwrap(), detached);
        let request: Request = serde_json::from_value(json!({
            "id": 12,
            "method": "session.load",
            "params": {"session_id": "s"}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::SessionLoad { ref session_id, .. } if session_id == "s"
        ));

        let request: Request = serde_json::from_value(json!({
            "id": 5,
            "method": "session.set_config_option",
            "params": {"session_id": "s", "config_id": "mode", "value": "plan"}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::SessionSetConfigOption { value: ConfigValue::Id(ref id), .. } if id == "plan"
        ));
        let request: Request = serde_json::from_value(json!({
            "id": 6,
            "method": "session.set_config_option",
            "params": {"session_id": "s", "config_id": "fast", "value": true}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::SessionSetConfigOption {
                value: ConfigValue::Boolean(true),
                ..
            }
        ));

        let accept = json!({
            "id": 19,
            "method": "elicitation.respond",
            "params": {
                "session_id": "s",
                "request_id": "2",
                "action": "accept",
                "content": {"color": "blue", "count": 3, "extras": ["nuts"], "rush": true}
            }
        });
        let request: Request = serde_json::from_value(accept.clone()).unwrap();
        let Method::ElicitationRespond {
            ref session_id,
            ref request_id,
            action: ElicitationAction::Accept(ref accepted),
        } = request.method
        else {
            panic!("{:?}", request.method);
        };
        assert_eq!((session_id.as_str(), request_id.as_str()), ("s", "2"));
        let content = accepted.content.as_ref().unwrap();
        assert_eq!(content["count"], ElicitationContentValue::Integer(3));
        assert_eq!(
            content["extras"],
            ElicitationContentValue::StringArray(vec!["nuts".into()])
        );
        assert_eq!(serde_json::to_value(&request).unwrap(), accept);
        for action in ["decline", "cancel"] {
            let value = json!({
                "id": 20,
                "method": "elicitation.respond",
                "params": {"session_id": "s", "request_id": "2", "action": action}
            });
            let request: Request = serde_json::from_value(value.clone()).unwrap();
            assert!(matches!(
                request.method,
                Method::ElicitationRespond {
                    action: ElicitationAction::Decline | ElicitationAction::Cancel,
                    ..
                }
            ));
            assert_eq!(serde_json::to_value(&request).unwrap(), value);
        }

        let method = Method::RunStart {
            workspace_id: "w".into(),
            workflow: "plan-work".into(),
            input: "Fix the build".into(),
            env: BTreeMap::from([("PATH".into(), "/bin".into())]),
        };
        let value = serde_json::to_value(Request { id: 25, method }).unwrap();
        assert_eq!(
            value,
            json!({
                "id": 25,
                "method": "run.start",
                "params": {
                    "workspace_id": "w",
                    "workflow": "plan-work",
                    "input": "Fix the build",
                    "env": {"PATH": "/bin"}
                }
            })
        );
        let request: Request = serde_json::from_value(json!({
            "id": 26,
            "method": "run.start",
            "params": {"workspace_id": "w", "workflow": "plan-work", "input": ""}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::RunStart { ref workflow, ref input, ref env, .. }
                if workflow == "plan-work" && input.is_empty() && env.is_empty()
        ));
        let value = serde_json::to_value(Request {
            id: 27,
            method: Method::RunCancel { run_id: "r".into() },
        })
        .unwrap();
        assert_eq!(
            value,
            json!({"id": 27, "method": "run.cancel", "params": {"run_id": "r"}})
        );
        let request: Request = serde_json::from_value(value).unwrap();
        assert!(matches!(
            request.method,
            Method::RunCancel { ref run_id } if run_id == "r"
        ));
        let request: Request = serde_json::from_value(json!({
            "id": 28,
            "method": "run.delete",
            "params": {"run_id": "r"}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::RunDelete { ref run_id, discard: false, ref env }
                if run_id == "r" && env.is_empty()
        ));
        let method = Method::RunDelete {
            run_id: "r".into(),
            discard: true,
            env: BTreeMap::new(),
        };
        let value = serde_json::to_value(Request { id: 29, method }).unwrap();
        assert_eq!(
            value,
            json!({
                "id": 29,
                "method": "run.delete",
                "params": {"run_id": "r", "discard": true, "env": {}}
            })
        );

        let value = serde_json::to_value(Request {
            id: 31,
            method: Method::SessionPostStatusUpdate {
                session_id: "s".into(),
                text: "Reading the code".into(),
            },
        })
        .unwrap();
        assert_eq!(
            value,
            json!({
                "id": 31,
                "method": "session.post_status_update",
                "params": {"session_id": "s", "text": "Reading the code"}
            })
        );
        let request: Request = serde_json::from_value(value).unwrap();
        assert!(matches!(
            request.method,
            Method::SessionPostStatusUpdate { ref session_id, ref text }
                if session_id == "s" && text == "Reading the code"
        ));

        let value = serde_json::to_value(Request {
            id: 30,
            method: Method::WorkflowList,
        })
        .unwrap();
        assert_eq!(value, json!({"id": 30, "method": "workflow.list"}));
        let request: Request = serde_json::from_value(value).unwrap();
        assert!(matches!(request.method, Method::WorkflowList));
        let workflows = vec![
            WorkflowInfo {
                workflow: "plan-work".into(),
                name: "Plan and work".into(),
                error: None,
            },
            WorkflowInfo {
                workflow: "broken".into(),
                name: "broken".into(),
                error: Some("missing field `steps`".into()),
            },
        ];
        let value = serde_json::to_value(&workflows).unwrap();
        assert_eq!(
            value,
            json!([
                {"workflow": "plan-work", "name": "Plan and work", "error": null},
                {"workflow": "broken", "name": "broken", "error": "missing field `steps`"}
            ])
        );
        assert_eq!(
            serde_json::from_value::<Vec<WorkflowInfo>>(value).unwrap(),
            workflows
        );

        let value = serde_json::to_value(Request {
            id: 1,
            method: Method::Hello {
                protocol_version: 1,
            },
        })
        .unwrap();
        assert_eq!(
            value,
            json!({"id": 1, "method": "hello", "params": {"protocol_version": 1}})
        );
    }

    /// An elicitation's events, delivered to subscription 6.
    fn elicitation_events() -> Vec<ServerMessage> {
        let requested: EventKind = serde_json::from_value(json!({
            "type": "elicitation_requested",
            "data": {
                "request_id": "1",
                "message": "Order",
                "tool_call_id": "tool-1",
                "requested_schema": {
                    "type": "object",
                    "properties": {"note": {"type": "string", "title": "Note"}},
                    "required": ["note"]
                }
            }
        }))
        .unwrap();
        let resolved: EventKind = serde_json::from_value(json!({
            "type": "elicitation_resolved",
            "data": {"request_id": "1", "action": {"action": "accept", "content": {"note": "hi"}}}
        }))
        .unwrap();
        [requested, resolved]
            .into_iter()
            .zip(1..)
            .map(|(kind, seq)| ServerMessage::Event {
                subscription: 6,
                event: Box::new(Event {
                    session_id: "s".into(),
                    seq,
                    kind,
                }),
            })
            .collect()
    }

    #[test]
    fn server_message_round_trip() {
        let event = Event {
            session_id: "s".into(),
            seq: 2,
            kind: EventKind::TurnEnded {
                stop_reason: Some(StopReason::EndTurn),
                error: None,
            },
        };
        let messages = [
            ServerMessage::Result {
                id: 1,
                result: Value::Null,
            },
            ServerMessage::Error {
                id: 2,
                error: ErrorBody {
                    code: code::NOT_FOUND,
                    message: "no".into(),
                },
            },
            ServerMessage::Event {
                subscription: 3,
                event: Box::new(event),
            },
            ServerMessage::List {
                subscription: 4,
                changes: vec![ListingChange::SessionRemoved {
                    session_id: "s".into(),
                }],
            },
            ServerMessage::Directory {
                subscription: 6,
                entries: vec![
                    DirEntry {
                        name: "a.txt".into(),
                        kind: EntryKind::File,
                    },
                    DirEntry {
                        name: "src".into(),
                        kind: EntryKind::Directory,
                    },
                ],
            },
        ];
        let status_update = EventKind::StatusUpdate {
            text: "Reading the code".into(),
            posted_at: 1_700_000_000_000,
        };
        let value = serde_json::to_value(&status_update).unwrap();
        assert_eq!(
            value,
            json!({
                "type": "status_update",
                "data": {"text": "Reading the code", "posted_at": 1_700_000_000_000_u64}
            })
        );
        assert_eq!(
            serde_json::from_value::<EventKind>(value).unwrap(),
            status_update
        );
        for message in messages.into_iter().chain(elicitation_events()) {
            let line = serde_json::to_string(&message).unwrap();
            let parsed: ServerMessage = serde_json::from_str(&line).unwrap();
            assert_eq!(serde_json::to_string(&parsed).unwrap(), line);
        }

        let message: ServerMessage = serde_json::from_value(json!({
            "subscription": 5,
            "changes": [
                {
                    "type": "workspace",
                    "workspace": {"id": "w", "path": "/tmp", "git_working_tree": true}
                },
                {"type": "workspace_removed", "workspace_id": "v"},
                {"type": "worktree", "worktree": {
                    "id": "01234567-89ab",
                    "path": "/worktrees/tmp-01234567",
                    "branch": "ox/01234567",
                    "base": "abc",
                    "created_branch": true,
                    "compare": "main",
                    "comparison": {
                        "type": "compared",
                        "tip": "abc",
                        "compare_tip": "def",
                        "ahead": 1,
                        "behind": 2
                    }
                }},
                {"type": "worktree_removed", "worktree_id": "u"},
                {"type": "session", "session": {
                "id": "s",
                "workspace_id": "w",
                "command": ["agent"],
                "cwd": "/tmp",
                "state": "idle",
                "title": "Fix the build",
                "agent_session_id": "a",
                "prompt_capabilities": {},
                "config_options": [],
                "available_commands": [],
                "worktree_id": "01234567-89ab",
                "archived": false,
                "created_at": 1_700_000_000_000_u64
                }},
                {"type": "session_removed", "session_id": "r"},
                {"type": "run", "run": {
                    "id": "run",
                    "workspace_id": "w",
                    "workflow": "plan-work",
                    "name": "Plan and work",
                    "input": "Fix the build",
                    "worktree_id": "01234567-89ab",
                    "state": "failed",
                    "error": "boom",
                    "steps": [
                        {"name": "Plan", "state": "succeeded", "started_at": 1_700_000_000_002_u64,
                         "session_id": "s", "output": "planned",
                         "commands": [
                            {"kind": "precondition", "command": "test -z \"$(git status --porcelain)\"",
                             "state": "succeeded", "exit_code": 0, "output": "", "truncated": false},
                            {"kind": "postcondition", "command": "make check",
                             "state": "failed", "exit_code": null, "output": "...killed",
                             "truncated": true}
                         ]},
                        {"name": "Work", "state": "failed", "started_at": 1_700_000_000_003_u64,
                         "session_id": null, "output": null,
                         "commands": [
                            {"kind": "shell", "command": "make", "state": "running",
                             "exit_code": null, "output": "", "truncated": false}
                         ]},
                        {"name": "Review", "state": "pending", "started_at": null,
                         "session_id": null, "output": null,
                         "commands": []}
                    ],
                    "created_at": 1_700_000_000_001_u64
                }},
                {"type": "run_removed", "run_id": "q"}
            ]
        }))
        .unwrap();
        let ServerMessage::List {
            subscription: 5,
            changes,
        } = message
        else {
            panic!("not a list: {message:?}");
        };
        let [
            ListingChange::Workspace { workspace },
            ListingChange::WorkspaceRemoved { workspace_id },
            ListingChange::Worktree { worktree },
            ListingChange::WorktreeRemoved { worktree_id },
            ListingChange::Session { session },
            ListingChange::SessionRemoved { session_id },
            ListingChange::Run { run },
            ListingChange::RunRemoved { run_id },
        ] = &changes[..]
        else {
            panic!("unexpected changes: {changes:?}");
        };
        assert!(workspace.id == "w" && workspace.git_working_tree);
        assert_eq!(workspace_id, "v");
        assert_eq!(worktree_id, "u");
        assert_eq!(session_id, "r");
        assert_eq!(run_id, "q");
        assert_eq!(
            *run,
            RunInfo {
                id: "run".into(),
                workspace_id: "w".into(),
                workflow: "plan-work".into(),
                name: "Plan and work".into(),
                input: "Fix the build".into(),
                worktree_id: "01234567-89ab".into(),
                state: RunState::Failed,
                error: Some("boom".into()),
                steps: vec![
                    StepInfo {
                        name: "Plan".into(),
                        state: StepState::Succeeded,
                        started_at: Some(1_700_000_000_002),
                        session_id: Some("s".into()),
                        output: Some("planned".into()),
                        commands: vec![
                            CommandInfo {
                                kind: CommandKind::Precondition,
                                command: r#"test -z "$(git status --porcelain)""#.into(),
                                state: CommandState::Succeeded,
                                exit_code: Some(0),
                                output: String::new(),
                                truncated: false,
                            },
                            CommandInfo {
                                kind: CommandKind::Postcondition,
                                command: "make check".into(),
                                state: CommandState::Failed,
                                exit_code: None,
                                output: "...killed".into(),
                                truncated: true,
                            },
                        ],
                    },
                    StepInfo {
                        name: "Work".into(),
                        state: StepState::Failed,
                        started_at: Some(1_700_000_000_003),
                        session_id: None,
                        output: None,
                        commands: vec![CommandInfo {
                            kind: CommandKind::Shell,
                            command: "make".into(),
                            state: CommandState::Running,
                            exit_code: None,
                            output: String::new(),
                            truncated: false,
                        }],
                    },
                    StepInfo {
                        name: "Review".into(),
                        state: StepState::Pending,
                        started_at: None,
                        session_id: None,
                        output: None,
                        commands: Vec::new(),
                    },
                ],
                created_at: 1_700_000_000_001,
            }
        );
        let value = serde_json::to_value(run).unwrap();
        assert_eq!(serde_json::from_value::<RunInfo>(value).unwrap(), *run);
        assert_eq!(session.workspace_id, "w");
        assert_eq!(session.title.as_deref(), Some("Fix the build"));
        assert_eq!(session.worktree_id.as_deref(), Some("01234567-89ab"));
        assert_eq!(session.created_at, 1_700_000_000_000);
        let value = serde_json::to_value(session).unwrap();
        assert_eq!(
            serde_json::from_value::<SessionInfo>(value).unwrap(),
            **session
        );
        assert!(
            worktree.id == "01234567-89ab"
                && worktree.branch == "ox/01234567"
                && worktree.created_branch
                && worktree.compare.as_deref() == Some("main")
        );
        assert_eq!(
            worktree.comparison,
            Some(Comparison::Compared {
                tip: "abc".into(),
                compare_tip: "def".into(),
                ahead: 1,
                behind: 2,
            })
        );
        let failed = Comparison::Failed {
            error: "main is not a commit".into(),
        };
        let value = serde_json::to_value(&failed).unwrap();
        assert_eq!(
            value,
            json!({"type": "failed", "error": "main is not a commit"})
        );
        assert_eq!(serde_json::from_value::<Comparison>(value).unwrap(), failed);
    }
}
