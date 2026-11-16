# Fold a completed turn's work under "Worked"

## Goal

**Brief:** `agents/briefs/2026-11-16-003-worked-groups.md`

## Related code

- `crates/ox/src/conversation.rs` — `Conversation::apply` folds events into
  the items the UI renders. `EventKind::TurnEnded` yields a `TurnError` for an
  error and nothing for `EndTurn`, `Cancelled` or no stop reason, so the UI
  cannot tell a completed turn from a running or cancelled one. Its tests
  (`chunks_belong_to_their_message`, `user_messages_are_prompts_only_outside_a_turn`,
  `a_turn_ends_as_an_error_with_its_details`) fold `turn_ended()`.
- `ui/src/lib/protocol.ts` — the `Item` union the UI receives.
- `ui/src/features/conversation/Conversation.tsx` — `Conversation` maps items
  straight to memoized `ItemView`s; `ItemView` decides what renders nothing
  (blank thoughts, permission and elicitation items).
- `ui/src/features/runs/RunStatus.tsx` — the run view's step toggle: a
  `<button aria-expanded>` with a trailing `ChevronRight` that rotates 90° when
  open (`ui/src/styles/run-view.css`, `.run-status-toggle`), rendering its
  content only while open. The Worked toggle follows it.
- `ui/src/styles/conversation.css` — transcript layout and the thought and
  tool call styles.
- Hypercube's `internal/ui/web/transcript/activity-item.tsx` — the reference:
  muted "Worked" label with a chevron after it, a rule beneath, the activity
  below the rule when open; no group when the turn showed nothing before its
  reply.
- `crates/oxd/src/bin/oxd-fake-agent.rs` — the e2e agent. `thought`,
  `messages`, `tool` and `text` directives give a turn a thought, a mid-turn
  message, a tool call and a final reply in that order; `fail` ends it with an
  error.
- `ui/e2e/specs/stress.e2e.ts` — `bigWorkspace` builds the long transcript as
  one turn of `messages N`, and `openLong` waits for N `.conversation > .text`
  elements.
- `ui/mock/scenarios.ts` — the `sidebar` scenario's `Fix sidebar colors`
  session holds one running turn.

## Decisions

- **The core marks a completed turn with a new item, `TurnCompleted { key }`**
  (`turn_completed` in TypeScript), emitted for a `TurnEnded` with no error and
  a stop reason of `EndTurn` or none — the cases that today yield no item and
  are not a cancellation. Its key is the event's seq, like `TurnError`. It
  renders nothing. Every other end already yields `TurnError`, `AgentExited`,
  or nothing (`Cancelled`), so those turns stay expanded without further work.
  Deciding in the core keeps the stop-reason knowledge where the other turn-end
  mapping lives, and saved conversations replay the same events, so they fold
  the same way.
- **The UI groups items into rows in one pure function, `rows(items)`, in
  `Conversation.tsx`.** A turn's work is the items after the previous boundary
  (a `prompt`, `turn_completed`, `turn_error`, `agent_exited` or
  `session_closed` item). At a `turn_completed`, the reply is the turn's last
  shown item; when it is a `text` item and at least one shown item precedes it,
  the items before the reply become one `worked` row and the reply and anything
  after it stay as item rows. Otherwise the turn's items stay as item rows. A
  turn the agent started itself (no prompt) folds the same way.
- **One predicate, `shown(item)`, says whether `ItemView` renders anything**,
  and both `ItemView` and `rows` use it: blank `text` and `thought`,
  `permission`, `elicitation` and `turn_completed` are not shown. `ItemView`
  returns `null` for an item that is not shown, replacing its inline blank
  thought check.
- **`Worked` is a memoized component, `WorkedGroup`, keyed `worked:<first item
  key>`**, with its open state in `useState`, closed by default. It renders a
  `<button className="worked-toggle" aria-expanded>` reading "Worked" with a
  `ChevronRight` after it, a rule beneath, and the work's `ItemView`s only
  while open. Its memo compares the `items` arrays element by element, so a
  later turn's streaming reply does not render earlier groups again.
- **`Conversation` computes `rows` in `useMemo` on `items`.** A turn's items
  render flat while it runs and move under the group when its
  `turn_completed` arrives; they remount there, which is only seen once the
  group is opened.
- **Styles follow Hypercube within Ox's tokens:** the toggle is
  `--muted-foreground`, gap `var(--spacing) * 2` between label and chevron,
  chevron sized `--icon-size` with the run view's 150 ms rotation; the rule is
  `border-top: var(--border-width) solid var(--border)` a spacing unit below
  the toggle; open content is a column with the transcript's
  `var(--spacing) * 3` gap, padded `var(--spacing) * 4` above. The toggle has
  no button chrome.
- **The stress fixture's long turn ends on a tool call**
  (`messages N …\ntool end`), so it ends without a reply, stays expanded, and
  `openLong` keeps measuring thousands of rendered replies.

## Check your assumptions

1. **Whether an opened Worked group stays open across session switches and restarts.** — Kept.
   - **Decided:** It stays open only while the session stays on screen; switching sessions or restarting the app closes it. `SessionContent` is keyed by session ID (`ui/src/App.tsx`), so the conversation remounts on a switch, which already closes opened thoughts and tool calls.
   - **Consequences:** After switching away and back, every group is closed again.
   - **Alternative:** Keep each session's opened groups in app-level state for the app's lifetime, or save them.
   - **Trade-offs:** Closing matches thoughts and tool calls and needs no state outside the component; remembering adds a store keyed by session and group, and would leave thoughts and tool calls inconsistent unless they move too.
2. **What a turn whose agent replied and then kept working counts as.** — Kept.
   - **Decided:** The reply is the turn's last shown item. When that is a thought or tool call rather than text, the turn ended without a reply and stays expanded. The existing Markdown e2e test streams a thought after the message it belongs to, so this order does occur.
   - **Consequences:** Such a turn shows everything in place, like an errored one.
   - **Alternative:** Treat the turn's last text item as the reply wherever it sits, and fold everything else, including what came after it.
   - **Trade-offs:** The decided rule never hides work below the visible reply and keeps the prompt–group–reply order; the alternative folds more turns but can show the reply above work that came after it.
3. **Turns in replayed agent history.** — Kept.
   - **Decided:** They stay expanded. A session whose saved history had no agent start recovers its conversation through the agent's replay (`State::load` in `crates/oxd/src/state.rs`), which carries user and agent messages but no turn ends, so no `turn_completed` items exist for it.
   - **Consequences:** A restored session shows its recovered turns unfolded; turns run after the restore fold.
   - **Alternative:** Treat each replayed user message as the end of the turn before it and fold those turns too.
   - **Trade-offs:** The decided rule needs no inference and can't fold a turn that actually failed; the alternative folds restored history but guesses where turns ended and whether they succeeded.
4. **How the stress test keeps measuring a long transcript.** — Kept.
   - **Decided:** Its long turn ends on a tool call, so it stays expanded and the test still renders thousands of replies.
   - **Consequences:** The stress fixture is a turn that ends without a reply, not the folded case.
   - **Alternative:** Leave the fixture folding and have `openLong` open the Worked group and measure that.
   - **Trade-offs:** The decided change keeps the test's existing measure of opening a long session; the alternative measures opening a huge group but times a click, not the session switch.

## Test plan

- `crates/ox/src/conversation.rs`: a new table test, `a_completed_turn_is_marked`,
  folds `TurnEnded` with `EndTurn`, no stop reason, `Cancelled`, an error stop
  reason, and an error string, and expects `TurnCompleted` for the first two,
  nothing for `Cancelled`, and `TurnError` for the rest. Update
  `chunks_belong_to_their_message` and
  `user_messages_are_prompts_only_outside_a_turn` to expect the trailing
  `TurnCompleted`.
- `ui/e2e/specs/session.e2e.ts`, "folds a completed turn's work under Worked":
  send `thought "Planning"`, `messages 1 "Checking the tests"`, `tool listed`,
  `text "Done"`. Once `.conversation > .worked .worked-toggle` exists, assert it
  reads "Worked" with `aria-expanded="false"`, the conversation's children are
  the prompt, the group, and the reply "Done" in that order, `agentText()` has
  one element, and no `.tool-call` or `.thought` exists. Click the toggle and
  assert the thought, "Checking the tests" and the tool call show inside the
  group. `reload()` and assert the group is closed again with the same order.
- Same spec, "keeps a turn that ends in an error expanded": send
  `thought "Planning"`, `tool listed`, `fail broken`; wait for `.turn-error`,
  then assert no `.worked` exists and the tool call shows.
- `ui/e2e/specs/stress.e2e.ts` passes with its fixture ending on `tool end`.
- The rest of the e2e suite passes unchanged: its turns either have no work
  before a single reply or end on a tool call or thought.
- Mock mode: the `sidebar` scenario's `Fix sidebar colors` session shows a
  folded completed turn above its running turn; compare the toggle and rule
  with the tool call rows and the run view toggle for size, colour, spacing and
  alignment, closed and open.

## Implementation plan

1. `crates/ox/src/conversation.rs`: add `Item::TurnCompleted { key }` (doc: a
   turn the agent ended on its own without an error), include it in
   `Item::key`, and return it from the `TurnEnded` arm for
   `(None, Some(StopReason::EndTurn) | None)`, keeping `Cancelled` empty. Add
   and update the tests above.
2. `ui/src/lib/protocol.ts`: add `{ type: "turn_completed"; key: string }` to
   `Item`.
3. `ui/mock/scenarios.ts`: in the `sidebar` scenario, put a completed turn
   before the running one in `Fix sidebar colors`: a prompt, a thought, a
   mid-turn text, two completed tool calls, the reply text, and a
   `turn_completed`, with keys that keep the running turn's items after them.
4. `ui/src/features/conversation/Conversation.tsx`: add `shown`, `Row`,
   `rows` and `WorkedGroup` as decided; render `rows` in `Conversation`;
   return `null` from `ItemView` for items that are not shown. ES2022 has no
   `findLastIndex`, so find the reply with a loop.
5. `ui/src/styles/conversation.css`: add `.worked`, `.worked-toggle`, its
   chevron and its open state, and the rule and open content.
6. `ui/e2e/specs/session.e2e.ts`: add the two tests.
7. `ui/e2e/specs/stress.e2e.ts`: end the long turn with `tool end` and say why
   in the fixture's comment.

## Documentation updates

- `README.md`: extend the "Streaming responses, thinking, tool calls, and
  background-task updates" feature to say a completed turn's work folds under
  Worked, leaving the prompt and reply.
- `docs/dev/performance.md` (UI): a completed turn's work renders only while
  its Worked group is open, and a group renders again only when its own items
  change.
