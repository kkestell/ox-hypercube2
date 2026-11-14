# Code style

Build a clear, coherent solution to the actual problem. Spend effort on the
required behavior and everyday use. Avoid speculative architecture and chasing
every last edge case.

## Scope and architecture

- Judge the whole design by how easy it is to understand and change. Restructure
  or replace existing code when that produces a better solution; minimizing the
  diff and reusing existing machinery are not goals.
- Give responsibilities clear boundaries. Introduce types, modules, and
  abstractions when they make the current problem easier to express and reason
  about.
- Do not add abstractions, manifests, protocols, orchestration layers,
  configuration, pagination, fallback paths, extension points, or future phases
  for hypothetical needs.
- Plans contain only work required now. Include speculative follow-up
  architecture only when explicitly requested.
- Base generalization on concrete uses. Possible future callers, backends, or
  integrations do not justify supporting machinery today.
- Do not turn routine implementation choices into specification requirements.
  Resolve ordinary details in code; discuss changes to observable behavior or
  scope when necessary.

## Implementation

- Write idiomatic Rust. Prefer plain data, direct functions, and explicit
  control flow. Keep responsibilities clear without turning every step into an
  abstraction.
- Handle invalid input and failures likely to occur in everyday use clearly.
- If an edge case is unlikely to occur in everyday use, add a local `TODO`
  comment naming the case and defer handling it until it actually occurs. Do
  not build recovery paths, defensive wrappers, or test matrices for it now.
- Trust internal invariants; fail clearly when they break. Avoid redundant
  checks, speculative recovery, and layers of defensive wrappers.
- Implement required semantics correctly. Simplicity does not justify leaving
  requested behavior incomplete.
- Measure before optimizing. Fix demonstrated problems rather than inventing
  them.

## Verification

See [Testing](testing.md) for test design and verification procedures.
