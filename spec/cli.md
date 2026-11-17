# Command-line access

`oxctl` lets users and scripts operate the same workspaces and sessions as the desktop. Commands that change a session do not require the desktop to be open.

## Workflows

### Connect

Every command connects to the session service, starting it when necessary. `--socket PATH` is a global option that selects the endpoint. Without it, `OX_SOCKET` selects the endpoint; otherwise the default is `oxd.sock` under `$XDG_DATA_HOME/ox` when `XDG_DATA_HOME` is absolute, or `~/.local/share/ox` otherwise.

Selecting another endpoint selects a separate collection of workspaces and sessions. Connections are restricted to the service's operating-system user.

### Manage workspaces and sessions

| Command | Result |
| --- | --- |
| `workspace add PATH` | Resolves the path and adds the directory; prints its workspace ID. |
| `workspace ls` | Prints workspace ID and absolute path, tab-separated, one workspace per line. |
| `workspace rm WORKSPACE` | Removes an empty workspace. |
| `ls` | Prints each session's ID, workspace ID, state, directory, agent command, title, and agent session ID, tab-separated. Missing title or agent session ID is `-`; archived sessions show `archived` as state. |
| `new --workspace WORKSPACE [OPTIONS] [-- COMMAND ARG ...]` | Creates a session, waits for the agent to open it, and prints the session ID. The default command is `claude-code-acp`. |
| `load SESSION` | Resumes an exited session, waits for startup, and prints its unchanged session ID. |
| `archive SESSION [--discard]` | Archives the session. |
| `unarchive SESSION` | Restores the archived session without starting its agent. |
| `delete SESSION [--discard]` | Deletes the session; prints `kept branch NAME` if its branch is retained. |

`new` runs in the workspace directory unless one of `--worktree`, `--branch NAME`, or `--new-branch NAME` is supplied. These three options are mutually exclusive. `--start COMMIT` requires `--new-branch`. `--compare REF` requires a worktree-selection option. Their meanings and refusals are specified in [Git tasks](git.md).

If a newly created session's agent fails to open it, `new` deletes that failed session and reports the failure instead of printing an ID. A failed `load` retains the session and directs the user to `attach` for details.

### Prompt, configure, and respond

| Command | Result |
| --- | --- |
| `prompt SESSION TEXT [--image PATH ...] [--file PATH ...]` | Sends text and any attachments, returning after Ox accepts the prompt rather than after the turn ends. During a turn it steers the turn. Each attachment option may be repeated. |
| `queue add SESSION TEXT` | Queues a text prompt and prints its ID. |
| `queue ls SESSION` | Prints each queued prompt's ID and text, tab-separated, in order. |
| `queue edit SESSION PROMPT TEXT` | Replaces a queued prompt's text in its place. |
| `queue rm SESSION PROMPT` | Removes a queued prompt. |
| `queue send SESSION PROMPT` | Takes a prompt out of the queue and sends it now, as `prompt` does. |
| `config SESSION` | Prints each option's ID, current value, and comma-separated allowed values, tab-separated. |
| `config SESSION OPTION VALUE` | Sets an option, then prints the resulting configuration. Boolean options require `true` or `false`; selects take a value ID. |
| `permit SESSION REQUEST OPTION` | Answers the pending permission request with its offered option ID. |
| `answer SESSION REQUEST 'JSON_OBJECT'` | Accepts a form with the supplied answers. |
| `answer SESSION REQUEST --decline` | Declines the form. |
| `answer SESSION REQUEST --cancel` | Cancels the form. |
| `cancel SESSION` | Requests interruption of the current turn. |

`--image` reads and sends image content; its extension must be PNG, JPG/JPEG, GIF, or WebP, without case sensitivity. An unreadable or unsupported image fails locally. `--file` supplies a reference to an existing local path rather than embedding its contents. Text precedes images, which precede file references. Agent content restrictions follow [agents and settings](agents.md#prompt-content).

Form content must be a JSON object. Content, `--decline`, and `--cancel` are mutually exclusive; one is required. Unknown requests and options, invalid answers, and invalid session states produce errors as described in [permissions and questions](requests.md).

### Follow events

`attach SESSION [--after SEQ] [--json]` prints prior events and follows new ones. By default it starts before the first event; `--after` includes only events with a greater sequence number. A sequence beyond the session's last event is refused.

- In text mode, streamed agent reply chunks print as continuous text. Other events appear on separate lines prefixed by their sequence number. These identify prompts, thinking, tools, states, turn endings, agent exits, and parking.
- Permission requests show request IDs and offered option IDs; forms show request IDs and field names, types, and choices for use with `permit` and `answer`.
- `--json` prints one complete event per line, including its session ID, sequence number, and event content. It retains agent updates not displayed by the desktop.
- Attaching to an exited or archived session reads its history without starting its agent. The command continues waiting for new events; it ends normally when the session is deleted, not merely when the agent exits. Losing the connection ends it with an error.
- Concurrent attachments are independent and receive events in order. Reattaching with the last received sequence number continues after that event without replaying it.

### Compare and move branches

| Command | Result |
| --- | --- |
| `compare SESSION [REF]` | Sets a supplied comparison ref or refreshes the existing comparison. |
| `compare SESSION --clear` | Clears the comparison; mutually exclusive with REF. |
| `move-branch --workspace WORKSPACE BRANCH TO --expected COMMIT [--force]` | Moves the local branch under the [branch-move safeguards](git.md#move-a-branch-explicitly). |

`compare` prints the ref, branch commit, comparison commit, ahead count, and behind count, tab-separated. With no comparison ref it prints nothing. A comparison failure exits with the reason. Successful branch moves, archive and unarchive operations, prompt submissions, cancellation, permission answers, form answers, and workspace removal print no result text.

### Operate the session service

`oxd [--socket PATH] [--idle-exit-ms MS] [--park-after-ms MS]` can be run explicitly. Endpoint selection without `--socket` follows the same environment and default rules above.

- `oxd` prints `ready` when its endpoint is available. Starting another instance for an already-running endpoint reports `ready` and exits successfully rather than starting a competing service.
- With no clients or active agents, it exits after five seconds by default. Saved exited sessions do not prevent exit. `--idle-exit-ms` changes that duration; `--park-after-ms` changes the [parking delay](sessions.md#leave-a-session-running). Both take nonnegative integer milliseconds.
- Termination or interrupt stops active agents and retains the sessions. A hangup does not terminate the service.

## Rules

- Commands require the full workspace, session, request, and option identities shown by the relevant listings or events; titles are not substitutes for IDs.
- A successful `oxctl` command exits with status 0. Failures print an `oxctl:`-prefixed reason on standard error and exit nonzero. Invalid command usage is rejected before connecting.
- Operations submitted to the service are not cancellation-on-disconnect: closing the invoking client does not undo a session creation or management operation already accepted.
- The desktop and command line share session state. A pending request can be answered from either; a turn can continue after the prompting command exits.

## Related areas

- [Workspaces and navigation](workspaces.md): shared workspace behavior.
- [Sessions](sessions.md): lifecycle rules independent of client.
- [Git tasks](git.md): branch selection, comparison, and cleanup behavior.
