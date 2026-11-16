# Documentation and naming

Every fact has one authoritative home. Choose that home by the subject the
fact describes, and reference it from elsewhere instead of restating it.

## Ownership

Read the subject's owner before changing it. Use the project's existing layout;
one file can hold several of the roles below.

| Subject | Authoritative home |
| --- | --- |
| What the project is, its features, and installation | README |
| Intended behavior, scope, and acceptance criteria | Specification or functional requirements |
| Durable design, boundaries, and system-wide constraints | Architecture |
| Development tools and how to use them | Development guide |
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
- Link to the owning file and heading in place of copying its explanation,
  rules, commands, or configuration.
- When moving a fact or contract, remove the old copy and update references.
- Keep current-state documents accurate. Distinguish intended behavior from
  implemented behavior and verified results from assumptions.
- Historical records describe what was known or done at their date. Promote
  lasting decisions into their current authoritative home, so readers learn
  the current system from current documents.

## When to edit documentation

- Update the owner when a fact it owns changes.
- Update architecture when a change alters a boundary, durable design decision,
  or system-wide constraint. Update the testing guide when the test strategy or
  procedure changes.
- Correct stale names, broken examples, and other factual mistakes where they
  occur. Treat an obvious correction as a fix.
- Create a document or section only when information needs a home readers can
  find.
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
  maintainer could otherwise miss, and describe the current code.
- Code style holds the decisions that need judgment. Formatters, linters, and
  the compiler own the rules they enforce.

## Naming

- Use one canonical term for each domain concept across code, tests, user-facing
  text, and documentation.
- Introduce a new term only for a distinct concept, and use the established
  name when one exists.
- Give distinct concepts distinct names, and qualify a word such as "session"
  when it would otherwise cover unrelated lifecycles.
- Qualify ambiguous terms when the surrounding context does not settle their
  meaning: for example, "HTTP client", "ACP client", "session title", or
  "tool call title".
- Prefer names that identify the domain role over generic words such as
  "data", "result", "manager", or "helper" when a more precise name exists.
- Keep a glossary when recurring ambiguity warrants it. Define each term once,
  briefly, and record distinctions and reserved meanings there.
- When terminology changes, update its uses and authoritative definition
  together.
