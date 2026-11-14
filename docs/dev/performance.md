# Performance

Ox's work follows what changed. A change to one session, one setting or one
message costs the same with three sessions or three hundred, and with ten
replies or thousands.

## Daemon

A `watch` sends every workspace and session once, then only those added,
changed or removed. Each session counts the changes to its `SessionInfo`;
the watch remembers the count it last sent for each session and reads the
info only of sessions whose count moved. Workspaces are listed again, with
their `git_working_tree` worked out from the filesystem, when a workspace or
session is added or removed or a client connects or disconnects.

## Desktop core

The core sends the viewed session's conversation to the webview in batches:
each holds the items from the events received together, with an item merged
into the earlier one with its key. Replaying a long session is a few large
messages, and a message's streamed text chunks arrive joined.

## UI

- The listing keeps the identity of every workspace and session that did not
  change, and the conversation keeps the identity of every item that did not
  change.
- Session rows, the composer, the file tree, the Merge button, the
  conversation and each conversation item are memoized and render again only
  when their own props change. The hooks that feed them return the same
  objects and callbacks until their data changes, so typing a draft or a
  change to another session renders nothing in the transcript.
- A message's Markdown is parsed into top-level blocks, and each block renders
  on its own from that parse. Text appended to a message parses and renders
  only its last block again; the blocks before it keep their identity.
- A chosen model or effort level shows in the same render as the choice. It
  stays shown until the agent reports it, and goes back if the agent refuses
  it.

## Verification

The stress spec, `ui/e2e/specs/stress.e2e.ts`, holds the app to time budgets
with hundreds of sessions and a transcript of thousands of long Markdown
replies. [Testing](testing.md#ox-tests-and-fixtures) describes how it
measures.
