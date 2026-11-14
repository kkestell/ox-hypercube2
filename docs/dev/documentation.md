# Documentation and naming

Every fact has one authoritative home. Choose that home by the subject the
fact describes, and reference it from elsewhere instead of restating it.

## Ownership

Read the subject's owner before changing it. Use the project's existing layout;
the roles below do not require a separate file for every subject.

| Subject | Authoritative home |
| --- | --- |
| Installation and use | README or user guide |
| Intended behavior, scope, and acceptance criteria | Specification or functional requirements |
| Durable design, boundaries, and system-wide constraints | Architecture |
| Contributor setup, commands, environment, and operational gotchas | Development guide |
| Test strategy and required verification procedures | Testing guide |
| Conventions that require human judgment | Code style |
| Canonical domain terms and distinctions | Glossary, when needed |
| Repository workflow and document ownership | AGENTS.md |
| Future work, dependencies, and completion conditions | Roadmap or task list |
| Known defects and current limitations | Issue tracker or bug list |
| Implementation mechanics | Code |
| Local, non-obvious reasons | Comments beside the relevant code |
| Configuration values and executable checks | Configuration and tests |
| Dated evidence, work performed, and historical decisions | Research, plans, reviews, work logs, and Git history |

- Prefer code, configuration, and tests for facts they already express clearly.
  Document contracts and reasoning that those sources do not make apparent.
- Link to the owning file and heading. Do not copy its explanation, rules,
  commands, or configuration into another document.
- When moving a fact or contract, remove the old copy and update references.
- Keep current-state documents accurate. Distinguish intended behavior from
  implemented behavior and verified results from assumptions.
- Historical records describe what was known or done at their date. Promote
  lasting decisions into their current authoritative home; readers should not
  have to reconstruct the current system from old plans and reviews.

## When to edit documentation

- Update the owner when a fact it owns changes. Completing a task is not itself
  a reason to edit documentation.
- A local code change needs no architecture update unless it changes a boundary,
  durable design decision, or system-wide constraint. A test change needs no
  testing-guide update unless the strategy or procedure changes.
- Correct stale names, broken examples, and other factual mistakes where they
  occur. Do not treat an obvious correction as a new design decision.
- Create a document or section when information needs a home readers can find.
  Do not create reports, recaps, or parallel explanations merely to record that
  work happened.
- Record the current system directly. Keep superseded designs and change
  narratives in historical records.

## Writing

- State the fact first. Keep prose direct, concrete, and concise. In a list,
  give each bullet one fact or instruction.
- Include a reason when it prevents a likely misunderstanding or wrong future
  change. Omit rhetorical emphasis, repeated explanations, and transitions that
  add no information.
- Use links for related material instead of summaries of other documents.
- Comments explain a local surprise, an external rule, or a constraint a future
  maintainer could otherwise miss. Do not narrate obvious code, describe the
  previous implementation, or cite task plans as implementation authority.
- Keep code-style guidance for decisions that need judgment. Do not reproduce
  rules already enforced by formatters, linters, or the compiler.

## Naming

- Use one canonical term for each domain concept across code, tests, user-facing
  text, and documentation. Do not alternate synonyms for variety.
- Introduce a new term only for a distinct concept. Do not invent names for
  things that already have clear, established names.
- Give distinct concepts distinct names. Do not overload a word such as
  "session" for unrelated lifecycles without a qualifier.
- Qualify ambiguous terms when the surrounding context does not settle their
  meaning: for example, "HTTP client", "ACP client", "session title", or
  "tool call title".
- Prefer names that identify the domain role over generic words such as
  "data", "result", "manager", or "helper" when a more precise name exists.
- Keep a glossary when recurring ambiguity warrants it. Define each term once,
  briefly, and record distinctions and reserved meanings there.
- When terminology changes, update its uses and authoritative definition
  together. Do not leave competing names for readers to reconcile.
