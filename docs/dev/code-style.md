# Code style

Build a clear, coherent solution to the actual problem. Spend effort on the
required behavior and everyday use.

## Scope and architecture

- Judge the whole design by how easy it is to understand and change. Restructure
  or replace existing code when that produces a better solution, whatever the
  size of the diff.
- Give responsibilities clear boundaries. Introduce types, modules, and
  abstractions when they make the current problem easier to express and reason
  about.
- Add abstractions, manifests, protocols, orchestration layers, configuration,
  pagination, fallback paths, and extension points only for present needs.
- Plans contain only work required now. Include speculative follow-up
  architecture only when explicitly requested.
- Base generalization on concrete uses.
- Resolve routine implementation choices in code, outside the specification.
  Discuss changes to observable behavior or scope when necessary.

## Implementation

- Write idiomatic Rust. Prefer plain data, direct functions, and explicit
  control flow. Keep responsibilities clear with as few abstractions as the
  problem needs.
- Handle invalid input and failures likely to occur in everyday use clearly.
- If an edge case is unlikely to occur in everyday use, add a local `TODO`
  comment naming the case and defer handling it until it actually occurs.
- Trust internal invariants and fail clearly when they break, checking each
  once.
- Implement required semantics correctly and completely.
- Measure before optimizing, and optimize demonstrated problems.

## UI

- The UI is idiomatic React in `ui/src/`, with icons from `lucide-react`.
- The user designs the app's look. UI changes add only the CSS a layout needs
  to work, such as a scrolling region or `white-space: pre-wrap`, plus styling
  the user asks for or styling that makes an element match its peers.
- Stylesheets take every colour, size, spacing, radius, and duration from the
  tokens in `ui/src/styles/tokens.css`. Rules space elements with multiples of
  `--spacing`, such as `calc(var(--spacing) * 2)`. A component's own size,
  and any value several elements share, is a named token in `tokens.css`,
  derived from the base tokens.
- Elements of the same kind look the same everywhere. A new element takes its
  class and tokens from the existing elements of its kind, and the CSS states
  each shared value once, in one rule for the kind. Elements in neighbouring
  regions that nearly line up share an edge or a centre line exactly.
- A change that would make one element differ from its peers, or that nudges a
  value by a few pixels to line things up, points at a missing or wrong shared
  abstraction: a class for the kind of element, a token, or a shared layout
  component. Propose the abstraction and build it first, so the change lands
  in the shared rule.
