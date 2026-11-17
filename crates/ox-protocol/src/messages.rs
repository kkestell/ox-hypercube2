use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    /// Starts a watch: after the answer, the daemon sends every workspace,
    /// worktree, and session at once, then each one that is added,
    /// changes, or is removed. A workspace's info is worked out again each
    /// time a workspace or session is added or removed or a client connects
    /// or disconnects. The request's `id` names the watch in every message.
    #[serde(rename = "watch")]
    Watch,
    /// Adds a workspace. `path` must be an absolute path to a directory that
    /// no workspace has.
    #[serde(rename = "workspace.add")]
    WorkspaceAdd { path: PathBuf },
    /// Removes a workspace and deletes its sessions, which must all be
    /// archived; their branches are kept. A deleted session is not
    /// remembered, so importing the directory's Claude Code sessions again
    /// brings it back. Refused while a session that is not archived belongs
    /// to it.
    #[serde(rename = "workspace.remove")]
    WorkspaceRemove { workspace_id: String },
    /// Imports the interactive Claude Code sessions started in the
    /// workspace's directory that the daemon does not have, newest first,
    /// as archived, exited sessions with their transcripts' history. A
    /// session the daemon has, or had and deleted, is skipped. Each session
    /// is added as soon as its transcript is read. The Claude bridge runs
    /// with `env` over the daemon's environment. The answer, when the import
    /// is done, counts the sessions imported. Refused when the bridge cannot
    /// list the sessions; a transcript it cannot read is skipped.
    #[serde(rename = "workspace.import_claude_sessions")]
    WorkspaceImportClaudeSessions {
        workspace_id: String,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// The branch the workspace's checkout is on. Refused outside a Git
    /// working tree. Git runs with `env` over the daemon's environment, as
    /// for `session.new`.
    #[serde(rename = "workspace.branches")]
    WorkspaceBranches {
        workspace_id: String,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Starts an agent in the workspace's directory or in a new worktree of
    /// its Git repository, as `checkout` says. The result is the session in
    /// state `starting`; its `agent_started` or `agent_exited` event
    /// follows. A worktree's branch is checked, and created unless the
    /// checkout names an existing local branch, before the answer, and the
    /// worktree is added during the session's startup. Git runs with `env` over the daemon's
    /// environment, as the agent does.
    #[serde(rename = "session.new")]
    SessionNew {
        workspace_id: String,
        command: Vec<String>,
        #[serde(default)]
        env: BTreeMap<String, String>,
        #[serde(default)]
        checkout: Checkout,
        /// The session's first prompt, of text and resource links only. Its
        /// `prompt_started` event is recorded before the answer, and the
        /// session's title is set from its text. The agent gets it once it
        /// has started and `config` is applied.
        #[serde(default)]
        prompt: Option<Vec<ContentBlock>>,
        /// Option values the agent is set to once it has started, in order,
        /// each only where the agent reports a different value. The first
        /// prompt waits for the agent's answers; when it refuses a value, or
        /// reports no option with its ID, the first prompt is not sent and
        /// its turn ends with an error naming each failed setting.
        #[serde(default)]
        config: Vec<ConfigSetting>,
    },
    /// Restarts the agent of an exited session that has an
    /// `agent_session_id`, from the session's command and directory, which
    /// must still exist. The
    /// result is the session in state `starting`. The agent replays the
    /// conversation before its `agent_started` event. A session whose history
    /// holds no agent start and no prompt records the replay as `update`
    /// events; any other session, including one restored after an `oxd`
    /// restart, keeps its history and drops the replay.
    #[serde(rename = "session.load")]
    SessionLoad {
        session_id: String,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Sends a prompt now. On an idle session it starts a turn
    /// (`prompt_started`); during a turn (state `running`,
    /// `awaiting_permission` or `awaiting_input`) it steers it
    /// (`prompt_steered`), and the agent takes it in at its next
    /// opportunity. Either resumes a queue held after a cancelled or failed
    /// turn.
    #[serde(rename = "session.prompt")]
    SessionPrompt {
        session_id: String,
        /// ACP content blocks. Blocks other than text and resource links
        /// need the matching prompt capability of the agent.
        prompt: Vec<ContentBlock>,
    },
    /// Appends a prompt to the session's queue. The result is
    /// `{"prompt_id": "..."}`. The daemon starts the queue's first prompt
    /// whenever the session is idle with no steered prompt pending, one
    /// prompt per turn; a cancelled turn, or one that ends other than with
    /// `end_turn`, holds the queue until `session.prompt` or
    /// `session.queue_send`.
    #[serde(rename = "session.queue_add")]
    SessionQueueAdd {
        session_id: String,
        prompt: Vec<ContentBlock>,
    },
    /// Replaces a queued prompt in place. An ID no longer queued is
    /// `not_found`.
    #[serde(rename = "session.queue_edit")]
    SessionQueueEdit {
        session_id: String,
        prompt_id: String,
        prompt: Vec<ContentBlock>,
    },
    /// Removes a queued prompt. An ID no longer queued is `not_found`.
    #[serde(rename = "session.queue_remove")]
    SessionQueueRemove {
        session_id: String,
        prompt_id: String,
    },
    /// Removes a queued prompt and sends it as `session.prompt` does. A
    /// refused send leaves it queued.
    #[serde(rename = "session.queue_send")]
    SessionQueueSend {
        session_id: String,
        prompt_id: String,
    },
    #[serde(rename = "session.cancel")]
    SessionCancel { session_id: String },
    /// Sends a peer message from session `session_id` to session `to`. It
    /// starts a turn or steers the running one as `session.prompt` does,
    /// recorded with its sender in `from`, but leaves a held queue held. The
    /// agent gets the text after a header that names the sender. An `exited`
    /// recipient is loaded first, with the sender's environment, and the
    /// message is sent once its agent has started. Refused for the sender
    /// itself, an archived session, and one whose agent never started. The
    /// result is [`MessageSent`].
    #[serde(rename = "session.send_message")]
    SessionSendMessage {
        session_id: String,
        to: String,
        text: String,
    },
    /// A page of session `session_id`'s compact transcript: its prompts,
    /// agent messages, tool calls and turn ends, each an entry, latest
    /// last. The page holds the latest entries before seq `before`, or
    /// before the end, up to 40,000 characters. The result is
    /// [`TranscriptPage`].
    #[serde(rename = "session.transcript")]
    SessionTranscript {
        session_id: String,
        #[serde(default)]
        before: Option<u64>,
    },
    /// Asks the user, through a permission request on session `session_id`,
    /// to start a new session with `prompt` as its first
    /// prompt. `workspace` is a workspace's directory and defaults to the
    /// requester's; `worktree` runs it in a new worktree on `branch`, an
    /// existing local branch, or a new branch at `HEAD`; otherwise in the
    /// workspace's checkout, whose branch must be `branch` if given.
    /// `harness`, `model`, `effort` and `mode` are IDs Ox's config lists:
    /// `effort` is required for a model with effort levels and refused for
    /// one without, and `mode` likewise for a harness with modes.
    /// Everything is checked before the request, which records the
    /// [`SessionRequest`] the user approves. The session starts with the
    /// requester's environment. The result is its [`SessionInfo`], or
    /// `null` when the user declines or the request is cancelled.
    #[serde(rename = "session.request")]
    SessionRequest {
        session_id: String,
        prompt: String,
        #[serde(default)]
        workspace: Option<PathBuf>,
        worktree: bool,
        #[serde(default)]
        branch: Option<String>,
        harness: String,
        model: String,
        #[serde(default)]
        effort: Option<String>,
        #[serde(default)]
        mode: Option<String>,
    },
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
    /// if every commit on it outside the ancestry of the session's starting
    /// commit is on another branch, tag or remote branch, and kept
    /// otherwise; a branch that existed before the session is always kept. Git runs with `env` over the daemon's environment, as for
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
    /// `expected`, or when `to` does not descend from `expected`. Local
    /// branch `source_branch` must still be at `to` when the destination ref
    /// moves.
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
        source_branch: String,
        #[serde(default)]
        env: BTreeMap<String, String>,
    },
    /// Recomputes the worktree's comparison and answers the worktree.
    #[serde(rename = "worktree.refresh")]
    WorktreeRefresh { worktree_id: String },
    /// Sets the session's title, shown in the session list and saved with
    /// the session. Runs of whitespace become one space, and a title left
    /// empty is refused with `invalid_request`. Answers `null`.
    #[serde(rename = "session.set_title")]
    SessionSetTitle { session_id: String, title: String },
    /// Sets the session this connection shows in front of the user,
    /// replacing the connection's last focus, or clears it with `null`.
    /// Closing the connection clears it too. Focusing a session clears its
    /// `unseen`, and while any connection focuses a session, it gets no
    /// `unseen`. An unknown ID is refused with `NOT_FOUND` and leaves the
    /// connection unfocused. Answers `null`.
    #[serde(rename = "focus")]
    Focus { session_id: Option<String> },
}

/// How a session's turn ended while no client had it in focus, kept until a
/// client focuses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unseen {
    Finished,
    Error,
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

    /// The harness whose bridge `command` runs.
    pub fn of_command(command: &[String]) -> Option<Harness> {
        let name = Path::new(command.first()?).file_name()?;
        Self::ALL
            .into_iter()
            .find(|harness| name == harness.bridge())
    }
}

/// A session an agent asks the user to start, as the user approves it.
/// `model`, `effort` and `mode` are the names Ox's config gives them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRequest {
    pub harness: Harness,
    pub model: String,
    /// The name of the provider that serves the model, when the config gives one.
    pub provider: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub checkout: RequestedCheckout,
    pub prompt: String,
}

/// Where a requested session runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RequestedCheckout {
    /// The checkout of the workspace at `path`.
    Workspace { path: PathBuf },
    /// A new worktree on existing local branch `branch`, or on a new branch
    /// from `base`, the workspace's branch or else `HEAD`.
    Worktree {
        branch: Option<String>,
        base: String,
    },
}

/// Where a new session runs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Checkout {
    /// The workspace's directory.
    #[default]
    Workspace,
    /// A new worktree of the workspace's repository, in the same
    /// subdirectory the workspace is of its checkout. With `branch`, a local
    /// branch checked out in no worktree, the worktree is on that branch and
    /// starts at its tip; otherwise on a new branch `ox/<first 8 digits of
    /// the worktree ID>` that starts at `HEAD`. With `compare`, a ref such
    /// as `main` that must name a commit, the worktree publishes how its
    /// branch compares with the ref.
    Worktree {
        #[serde(default)]
        compare: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        branch: Option<String>,
    },
}

/// A worktree the daemon added for a session.
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

/// The result of `session.delete`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deleted {
    /// The branch the daemon created for the worktree, kept
    /// because it has commits on no other branch, tag or remote branch.
    pub kept_branch: Option<String>,
}

/// The result of `workspace.import_claude_sessions`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedSessions {
    /// The number of sessions imported.
    pub imported: u64,
}

/// A config option value for `session.new` to set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConfigSetting {
    pub config_id: String,
    pub value: ConfigValue,
}

/// The result of `session.send_message`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSent {
    /// Whether the message steered the recipient's running turn, rather
    /// than starting one.
    pub steered: bool,
}

/// The result of `session.transcript`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptPage {
    /// The page's entries, separated by blank lines.
    pub text: String,
    /// The seq to pass as `before` for the page of earlier entries, unset
    /// when there are none.
    pub before: Option<u64>,
}

/// The session that sent a peer message, with its title as of sending.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sender {
    pub session_id: String,
    pub title: Option<String>,
}

/// The result of `session.queue_add`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueAdded {
    pub prompt_id: String,
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

/// A workspace's branches.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Branches {
    /// The local branch the workspace's checkout is on, unset when its
    /// `HEAD` is detached or unborn.
    pub head: Option<String>,
}

/// The daemon's workspaces, in the order they were added, its worktrees, and
/// its sessions, in the order they were created.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Listing {
    pub workspaces: Vec<WorkspaceInfo>,
    pub worktrees: Vec<WorktreeInfo>,
    pub sessions: Vec<SessionInfo>,
}

/// One change a watch delivers. A workspace, worktree or session that is new
/// comes after every one listed before it; one that changed keeps its place.
/// Each batch lists removals first, sessions before worktrees, then
/// workspaces, worktrees and sessions, so a session's worktree arrives before
/// the session.
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
    /// How the session's turns ended while it was out of focus, until a
    /// client focuses it: `error` when a turn failed or the agent exited
    /// with an error, otherwise `finished` when a turn ended other than by
    /// cancellation.
    pub unseen: Option<Unseen>,
    /// The prompts waiting to start a turn each, in order.
    pub queue: Vec<QueuedPrompt>,
}

/// A prompt in a session's queue.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QueuedPrompt {
    pub id: String,
    pub prompt: Vec<ContentBlock>,
}

/// One entry in a session's history. `seq` starts at 1 and increases by one
/// per event within a session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub session_id: String,
    pub seq: u64,
    /// When `oxd` recorded the event, in milliseconds since the Unix epoch.
    pub recorded_at: u64,
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
    /// event of a session that starts, after the `prompt_started` of a first
    /// prompt given to `session.new`.
    AgentStarted {
        agent_session_id: String,
        prompt_capabilities: PromptCapabilities,
        config_options: Vec<SessionConfigOption>,
        available_commands: Vec<AvailableCommand>,
    },
    /// A prompt that started a turn. `from` is the sender of a peer
    /// message, whose text `prompt` holds without the header the agent got.
    PromptStarted {
        prompt: Vec<ContentBlock>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<Sender>,
    },
    /// A prompt sent during a turn. The agent takes it in at its next
    /// opportunity, or starts a turn of its own with it. `from` is as for
    /// `prompt_started`.
    PromptSteered {
        prompt: Vec<ContentBlock>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<Sender>,
    },
    /// The agent took in the steered prompt recorded at seq `prompt`.
    SteeredPromptTaken {
        prompt: u64,
    },
    /// The agent refused the steered prompt recorded at seq `prompt`, dropped
    /// it on cancellation, or exited before taking it in.
    SteeredPromptDropped {
        prompt: u64,
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
        /// The session the request asks the user to start, when the request
        /// is the daemon's own, for `session.request`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_request: Option<SessionRequest>,
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
                    compare: None,
                    branch: None
                },
                ..
            }
        ));
        let checkout = Checkout::Worktree {
            compare: Some("main".into()),
            branch: None,
        };
        let value = serde_json::to_value(&checkout).unwrap();
        assert_eq!(value, json!({"type": "worktree", "compare": "main"}));
        assert_eq!(serde_json::from_value::<Checkout>(value).unwrap(), checkout);
        let checkout = Checkout::Worktree {
            compare: None,
            branch: Some("topic".into()),
        };
        let value = serde_json::to_value(&checkout).unwrap();
        assert_eq!(
            value,
            json!({"type": "worktree", "compare": null, "branch": "topic"})
        );
        assert_eq!(serde_json::from_value::<Checkout>(value).unwrap(), checkout);
        let method = Method::BranchMove {
            workspace_id: "w".into(),
            branch: "main".into(),
            to: "abc".into(),
            expected: "def".into(),
            source_branch: "topic".into(),
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
                    "env": {"PATH": "/bin"}
                }
            })
        );
        let request: Request = serde_json::from_value(json!({
            "id": 22,
            "method": "branch.move",
            "params": {"workspace_id": "w", "branch": "main", "to": "abc", "expected": "def", "source_branch": "topic"}
        }))
        .unwrap();
        assert!(matches!(
            request.method,
            Method::BranchMove { ref branch, ref to, ref expected, ref source_branch, ref env, .. }
                if branch == "main" && to == "abc" && expected == "def" && source_branch == "topic" && env.is_empty()
        ));
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
            Method::WorkspaceBranches { ref workspace_id, ref env }
                if workspace_id == "w" && env.is_empty()
        ));
        let branches = Branches {
            head: Some("main".into()),
        };
        let value = serde_json::to_value(&branches).unwrap();
        assert_eq!(value, json!({"head": "main"}));
        assert_eq!(serde_json::from_value::<Branches>(value).unwrap(), branches);
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

    #[test]
    fn a_stored_prompt_without_a_sender_still_reads() {
        let kind: EventKind = serde_json::from_value(json!({
            "type": "prompt_started",
            "data": {"prompt": [{"type": "text", "text": "hi"}]}
        }))
        .unwrap();
        assert!(matches!(kind, EventKind::PromptStarted { from: None, .. }));
        let from = Sender {
            session_id: "s".into(),
            title: Some("Fix the build".into()),
        };
        let kind = EventKind::PromptSteered {
            prompt: Vec::new(),
            from: Some(from.clone()),
        };
        let value = serde_json::to_value(&kind).unwrap();
        assert_eq!(
            value["data"]["from"],
            json!({"session_id": "s", "title": "Fix the build"})
        );
        assert_eq!(serde_json::from_value::<EventKind>(value).unwrap(), kind);
    }

    #[test]
    fn a_harness_is_named_by_its_bridge() {
        for (command, harness) in [
            (
                vec!["/app/claude-code-acp".to_owned()],
                Some(Harness::Claude),
            ),
            (
                vec!["/app/pi-acp".to_owned(), "-v".to_owned()],
                Some(Harness::Pi),
            ),
            (vec!["/bin/agent".to_owned()], None),
            (Vec::new(), None),
        ] {
            assert_eq!(Harness::of_command(&command), harness, "{command:?}");
        }
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
                    recorded_at: 1_700_000_000_000 + seq,
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
            recorded_at: 1_700_000_000_000,
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
                "created_at": 1_700_000_000_000_u64,
                "unseen": "finished",
                "queue": [{"id": "q1", "prompt": [{"type": "text", "text": "Then test"}]}]
                }},
                {"type": "session_removed", "session_id": "r"},
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
        ] = &changes[..]
        else {
            panic!("unexpected changes: {changes:?}");
        };
        assert!(workspace.id == "w" && workspace.git_working_tree);
        assert_eq!(workspace_id, "v");
        assert_eq!(worktree_id, "u");
        assert_eq!(session_id, "r");
        assert_eq!(session.workspace_id, "w");
        assert_eq!(session.title.as_deref(), Some("Fix the build"));
        assert_eq!(session.worktree_id.as_deref(), Some("01234567-89ab"));
        assert_eq!(session.created_at, 1_700_000_000_000);
        assert_eq!(session.unseen, Some(Unseen::Finished));
        assert_eq!(session.queue[0].id, "q1");
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
