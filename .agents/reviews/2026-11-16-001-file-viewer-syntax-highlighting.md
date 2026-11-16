# File viewer syntax highlighting review

## Scope

Reviewed the staged change against the
[plan](../plans/2026-11-16-001-file-viewer-syntax-highlighting.md) and
[work log](../work/2026-11-16-001-file-viewer-syntax-highlighting.md):
`highlight.ts`, `FileViewer.tsx`, `files.css`, `tokens.css`, the new E2E case in
`files.e2e.ts`, and the `lowlight` dependency. No code was changed during review.

## Findings

### 1. The viewer's text shrinks from `--text-sm` to `--text-xs`

`ui/src/styles/base.css:32` sets `font-size: var(--text-xs)` on every `code`
element. The viewer now renders `<pre><code>…</code></pre>`
(`ui/src/features/files/FileViewer.tsx:57`), so that rule applies directly to the
`<code>` and overrides the `--text-sm` that `.file-body pre`
(`ui/src/styles/files.css:67`) sets. Every file, highlighted or plain, now renders
at 12px instead of 14px, and its line height scales down with it. The plan
requires the viewer's existing presentation to stay unchanged. No test checks font
size, so the E2E suite passes anyway.

Proposed fix: render `<pre>{highlighted}</pre>` without the `<code>` wrapper. The
plan's `<pre><code>` choice has no other purpose here. Alternatively, add
`font-size: inherit` for `.file-body code`.

### 2. Light-mode token contrast is low; the visual check was not done

The plan asks for a light and dark comparison of every token against `--surface`.
The work log says it was not done. Computed WCAG contrast against `--surface`
(oklch → sRGB from the values in `tokens.css`):

| Token | Light | Dark |
| --- | --- | --- |
| plain text (`--text`) | 10.39 | 12.80 |
| `--syntax-keyword` | 3.76 | 7.19 |
| `--syntax-string` | 3.22 | 8.53 |
| `--syntax-constant` | 3.19 | 8.84 |
| `--syntax-type` | 3.13 | 6.07 |
| `--syntax-function` | 19.79 | 18.17 |
| `--syntax-comment` | 4.73 | 7.31 |
| `deletion` (`--destructive`) | 4.76 | 6.56 |

In light mode, keyword, string, constant and type fall below 4.5:1 at 14px
monospace, while the body text they replace reaches 10.4:1. `--syntax-function`
is `--foreground`, which sits next to the body's `--text` grey. Titles, sections
and attributes therefore look almost the same as plain text in both modes. The
plan calls the palette a starting point for the user to tune. These numbers
replace neither the visual check nor the user's choice of colours.

### 3. Opening a large file stalls the UI noticeably (open decision)

The work log measured `ui/package-lock.json` at 186 ms to open and 159 ms on
session restore. Highlighting runs in render, and `useMemo` lasts only as long as
each `FileViewer` mount, so every restore highlights again. As the plan
specified, the implementation records this as a `TODO` in `highlight.ts` and adds
no mitigation. In a coding tool, opening a lockfile or generated JSON is an
everyday action, while the code-style `TODO` rule covers unlikely edge cases.
This needs the user's decision on a mitigation (size cap, worker, or caching the
highlighted tree per snapshot) or acceptance of the cost.

## Plan conformance

- The language lookup (extension, then basename, both through
  `lowlight.registered`), the module-level instance, the memo on path and text,
  the six tokens set from existing colours, and the `.file-body`-scoped rules
  match the plan. The installed lowlight 3.3.0 / highlight.js 11.11.2 registers
  `rs ts tsx js py md yml toml mk makefile json css xml html sh bash`, and it does
  not register `lock` or the empty string.
- The CSS orders the function rule before the type rule, the reverse of the
  plan's table. This order is required: `:is()` gives both rules the same
  specificity, so `.hljs-title.class_` takes the type colour only because the
  type rule comes later.
- The plan said to extend the existing live-update test. The edit-highlighting
  assertion is in the new test instead, with equivalent coverage.
- Dark mode follows because the syntax tokens and their overrides are both
  declared on `:root`.

## Checks

- `make check-ui`: passed.
- `make e2e`: 13 of 13 spec files passed (`/tmp/ox-review-e2e.log`).
- `npm audit`: the 25 vulnerabilities (11 moderate, 14 high) are the same count at
  `HEAD`'s lockfile. None is in `lowlight`, `highlight.js`, or their
  dependencies. This change introduces none.
