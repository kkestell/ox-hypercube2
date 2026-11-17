# Agent instructions

Read the relevant guidance below before planning or changing code. These files
own their respective subjects; this file is the entry point and working
workflow.

## Guidance

| Document | Owns |
| --- | --- |
| [Code style](docs/dev/code-style.md) | Implementation, architecture, and scope decisions |
| [Development](docs/dev/development.md) | Development tools and how to use them |
| [Testing](docs/dev/testing.md) | Test design, verification procedures, and evidence of completion |
| [Documentation](docs/dev/documentation.md) | Document ownership, maintenance, writing, and naming rules |
| [Glossary](docs/dev/glossary.md) | Project vocabulary and distinctions between terms |

## Project documents

| Document | Owns |
| --- | --- |
| [README](README.md) | What Ox is, its features, and installation |
| [Architecture](docs/dev/architecture.md) | Process ownership, boundaries, persistence, and session lifecycle |
| [ACP v2 support](docs/dev/acp-v2.md) | Protocol support and integration caveats |
| [Where sessions run](docs/worktrees.md) | Checkout selection, branches, comparisons, and worktree cleanup |
| [Performance](docs/dev/performance.md) | How the daemon, the desktop core, and the UI keep work proportional to what changed |
| [Claude Code research](docs/research/harnesses/claude-code.md) and [Pi research](docs/research/harnesses/pi.md) | Historical harness observations |

## Working approach

- Establish the requested outcome and read the relevant code and project
  documents. Distinguish requirements from assumptions and possible future
  work.
- Resolve implementation decisions yourself. Ask when a missing decision
  changes what the user sees or does in everyday use of the desktop app, or
  the requested scope.
- Use a plan when the work needs design decisions or sequencing.
- Write every test to pass or fail on behavior alone, at any machine speed:
  it synchronizes on events and controls time as
  [Testing](docs/dev/testing.md#time-and-concurrency) describes.
- Run `make dev` as a background job and carry on without waiting for it; the
  user reports when the app is up, and Vite applies UI changes as they are saved.
- Work and commit directly on `main`. Do not create branches.
- Preserve unrelated working-tree changes. Commit or publish only when the user
  requests it.
- Complete verification according to [Testing](docs/dev/testing.md#running-checks) and
  reconcile changed facts according to
  [Documentation](docs/dev/documentation.md#when-to-edit-documentation).

## Installing changes

When the user asks to commit and install, commit on `main`, then:

- If the changes touch only `crates/ox/` and `ui/`, run `make install-ui`.
  It rebuilds and relaunches the app. `oxd` and every session, including
  this one, keep running.
- Otherwise, do not run `make install`. End the turn by telling the user to
  run `make install` in a terminal outside Ox once every session is idle.

Both targets build the working tree. When it holds changes that are not
committed, run the target in a temporary worktree of `HEAD`.

## Communication

- Lead with the substantive result. Use plain language and concrete names.
- Share findings, material uncertainty, blockers, and decisions that need the
  user. Skip opening acknowledgements and routine progress narration.
- Distinguish observations, conclusions, and assumptions. Claim an action
  succeeded once its result confirms it.
- When corrected, revisit the underlying judgment and apply the correction to
  the work, rather than patching only the example the user identified.
