# File viewer syntax highlighting work

Plan: `.agents/plans/2026-11-16-001-file-viewer-syntax-highlighting.md`.

- Added synchronous lowlight common-grammar highlighting, filename and extension lookup, and memoized React rendering. Unknown filenames remain plain text.
- Added the requested syntax colour tokens and scoped highlight.js rules. Added end-to-end coverage for Rust, TSX, Makefile, plain text, exact text, and a live highlighted edit.
- Confirmed the installed common set registers all languages and aliases listed in the plan.

## Verification

- `make check-ui` passed; `make e2e` passed (13 spec files), including existing restoration and scrolling coverage.
- In the desktop WebKit test app, a temporary measurement clicked a file and measured to the next animation frame after its text appeared; session restoration was measured from selection to the next frame. One run: `ui/package-lock.json` open 186 ms, restore 159 ms; `Cargo.lock` open 33 ms, restore 11 ms. Cargo.lock is plain text because `.lock` is not a registered language. The JSON delay is noticeable; a TODO records large-file stalls. A size cap, worker, or snapshot cache would need a separate decision.
- The requested visual comparison of every colour in light and dark mode was not completed; palette legibility remains unverified.
- `npm install` reported 25 dependency audit vulnerabilities (11 moderate, 14 high); no audit fixes were applied.
