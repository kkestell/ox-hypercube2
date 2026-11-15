# Agent instructions

Read the relevant guidance below before planning or changing code. These files
own their respective subjects; this file is the entry point and working
workflow.

## Guidance

| Document | Owns |
| --- | --- |
| [Code style](docs/dev/code-style.md) | Implementation, architecture, scope, and edge-case decisions |
| [Testing](docs/dev/testing.md) | Test design, verification procedures, and evidence of completion |
| [Documentation](docs/dev/documentation.md) | Document ownership, maintenance, writing, and naming rules |
| [Glossary](docs/dev/glossary.md) | Project vocabulary and distinctions between terms |

## Project documents

| Document | Owns |
| --- | --- |
| [README](README.md) | Project overview and development setup |
| [Architecture](docs/dev/architecture.md) | Process ownership, boundaries, persistence, and session lifecycle |
| [ACP v2 support](docs/dev/acp-v2.md) | Protocol support and integration caveats |
| [Where sessions run](docs/worktrees.md) | Checkout selection, branches, comparisons, and worktree cleanup |
| [Workflows](docs/workflows.md) | Workflow definitions, runs, step handoff, and run cleanup |
| [Performance](docs/dev/performance.md) | How the daemon, the desktop core, and the UI keep work proportional to what changed |
| [Claude Code research](docs/research/harnesses/claude-code.md) and [Pi research](docs/research/harnesses/pi.md) | Historical harness observations |

## Working approach

- Establish the requested outcome and read the relevant code and project
  documents. Distinguish requirements from assumptions and possible future
  work.
- Resolve ordinary implementation decisions yourself. Ask when a missing
  decision materially changes the requested outcome or scope.
- Use a plan when the work needs design decisions or sequencing. Routine work
  does not require a plan, report, or prescribed sequence of agent sessions.
- Preserve unrelated working-tree changes. Commit or publish only when the user
  requests it.
- Complete verification according to [Testing](docs/dev/testing.md#running-checks) and
  reconcile changed facts according to
  [Documentation](docs/dev/documentation.md#when-to-edit-documentation).

## Communication

- Lead with the substantive result. Use plain language and concrete names.
- Share findings, material uncertainty, blockers, and decisions that need the
  user. Skip opening acknowledgements and routine progress narration.
- Distinguish observations, conclusions, and assumptions. Do not claim an action
  succeeded until its result confirms it.
- When corrected, revisit the underlying judgment and apply the correction to
  the work, rather than patching only the example the user identified.
