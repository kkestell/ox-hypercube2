# Highlight syntax in the file viewer

## Goal

The file viewer colours source code by language. The language follows from the
file's name; a file whose language is not recognised shows as plain text, as
today. Highlighting follows live disk changes and keeps the viewer's existing
behaviour: exact text, no wrapping, selection and copying, scroll restoration,
and restoring a session's viewer before its first painted frame.

## Related code

- `ui/src/features/files/FileViewer.tsx` — renders `state.content.text` in a
  plain `<pre>`.
- `ui/src/styles/files.css` — `.file-body pre` rules.
- `ui/src/styles/tokens.css` — colour tokens, with dark-mode overrides.
- `ui/src/features/conversation/Markdown.tsx` — renders hast with
  `hast-util-to-jsx-runtime`, already a dependency.
- `ui/e2e/specs/files.e2e.ts` — the file viewer's end-to-end coverage, which
  asserts on `.file-body pre` text content.

## Decisions

- Highlight with `lowlight` (highlight.js grammars, hast output) and its
  `common` language set. It highlights synchronously, so the highlighted text is
  in the same render that restores a session's viewer before paint; an
  asynchronous highlighter would paint plain text first. Its output is a hast
  tree with `hljs-*` classes, which renders through the existing
  `hast-util-to-jsx-runtime` and takes its colours from tokens.
- Choose the language from the file name only: the basename's extension,
  lowercased, then the whole basename lowercased (so `Makefile` resolves), each
  checked with `lowlight.registered`, which accepts highlight.js aliases such
  as `rs`, `ts`, `tsx`, `py`, `md`, `yml`, `toml`, and `mk`. An unregistered
  name renders the text unhighlighted. Automatic language detection is out:
  it runs every grammar over the text and guesses wrong on prose.
- Put the lookup and highlighting in `ui/src/features/files/highlight.ts`,
  exporting one function that takes the path and text and returns the React
  nodes for the `<pre>`'s content. Create the lowlight instance once at module
  level. `FileViewer` calls it in a `useMemo` keyed on path and text, so
  scrolling and other viewer-state updates do not highlight again.
- Render `<pre><code>` with the highlighted nodes; the text content stays
  identical to the file's text.
- Colours: add six syntax tokens to `tokens.css`, each set from an existing
  colour token so dark mode follows without new overrides, and one rule per
  token in `files.css` scoped to `.file-body`. The palette is a starting point
  for the user to tune:

  | Token | Value | highlight.js classes |
  | --- | --- | --- |
  | `--syntax-keyword` | `var(--working)` | `keyword`, `selector-tag`, `doctag` |
  | `--syntax-string` | `var(--success)` | `string`, `regexp`, `char.escape_`, `addition` |
  | `--syntax-constant` | `var(--warning)` | `number`, `literal`, `symbol`, `bullet`, `link` |
  | `--syntax-type` | `var(--claude)` | `type`, `built_in`, `title.class_`, `name`, `tag` |
  | `--syntax-function` | `var(--foreground)` | `title`, `title.function_`, `section`, `attr`, `attribute`, `property` |
  | `--syntax-comment` | `var(--muted-foreground)` | `comment`, `quote`, `meta` |

  `deletion` uses `var(--destructive)` directly. Every other class keeps the
  `<pre>`'s colour. No background, weight, or italic changes.

## Check your assumptions

- After installing, confirm `lowlight` exports `common` and that it includes
  TypeScript, JavaScript, Rust, Python, Markdown, YAML, INI (TOML), JSON, CSS,
  XML, Bash, and Makefile. highlight.js 10.7's grammar files register the
  aliases above; confirm the installed version still does.

## Test plan

- E2E in `ui/e2e/specs/files.e2e.ts`, table-driven over paths that cover each
  lookup branch: `main.rs` (extension), `App.tsx` (alias), `Makefile`
  (basename), and `notes.txt` (unregistered). For each, open the file and
  assert the `<pre>` text equals the file's text exactly. For the highlighted
  ones, assert a keyword such as `fn`, `const`, or a Make target is inside an
  `.hljs-*` element whose computed colour differs from the `<pre>`'s; for
  `notes.txt`, assert there are no `.hljs-*` elements.
- Extend the existing live-update step: after an edit to a highlighted file
  adds a keyword, wait for the `<pre>` text to change, then assert the new
  keyword is highlighted.
- The existing viewer tests (exact text, `white-space: pre`, first-frame
  restoration, scroll restoration) must keep passing unchanged.
- Measure in the running app the render time of opening `ui/package-lock.json`
  (356 KB of JSON) and `Cargo.lock` (146 KB), and of switching back to a
  session showing one. Report the numbers. If either stalls the UI noticeably,
  add a `TODO` in `highlight.ts` naming large files and report it to the user;
  any mitigation (size cap, worker, caching the highlighted tree per snapshot)
  is a proposal for the user.
- Compare light and dark mode in the running app for legibility of every
  token colour against `--surface`.
- Run `make check-ui` and `make e2e`.

## Implementation plan

1. `cd ui && npm install lowlight`; confirm the assumptions above.
2. Add `ui/src/features/files/highlight.ts` with the language lookup and the
   hast-to-React rendering through `toJsxRuntime`.
3. Change `FileViewer.tsx` to render `<pre><code>{highlighted}</code></pre>`
   with the `useMemo` above.
4. Add the syntax tokens to `tokens.css` and the class rules to `files.css`.
5. Extend `ui/e2e/specs/files.e2e.ts` per the test plan, run the checks, and
   take the measurements.

## Documentation updates

None. No current document describes the viewer's rendering as plain text, and
the README's feature list does not cover the file viewer.
