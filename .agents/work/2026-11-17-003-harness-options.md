# Harnesses, models, and effort levels from a config file

## Plan

`.agents/plans/2026-11-17-003-harness-options.md`

## Summary

Ox reads the harnesses, models, effort levels, and modes it offers from its
config file; the bridges and the fake agent no longer describe themselves.
Drafts, running sessions, `session.request`, and the new `list_harnesses` tool
all use the config, and a refused setting fails a new session's first prompt.
The plan's goal is met.

## Departures from the plan

- **The config is Ox's general config file, in JSON.** By the user's
  correction during the work: `config.json` in Ox's config directory (by the
  plan's XDG rule), with the harness list under a `harnesses` key, so other
  settings can join it later. The module is `ox_protocol::config`, with
  `Config { harnesses }`, `config_dir`, `path`, and `load() -> Result<Config,
  String>`; the example is `config/config.json`, and `make install` copies it
  to `~/.config/ox/config.json` (or under an absolute `XDG_CONFIG_HOME`) when
  no file exists. `serde_json` replaces the planned `toml` dependency. The
  plan file was left for the coordinator to update.
- **No header comment in the example.** JSON has no comments; the format and
  the first-listed-is-default rule are in `config.rs`'s module doc, the
  architecture doc, and the README.
- **The Pi bridge lost its no-client paths.** Removing `--describe` left
  `Updates::without_client` and `Pi::start`'s `--no-session` case unused, so
  `Updates` holds its connection directly and `Pi::start` takes a session ID.
  Likewise `runner::commands` went with `Conversation::Unsaved`, and
  `Session::command` in `oxd`, which only the old default-harness lookup used.

## Decisions

- Refusal messages, all naming what the config lists:
  `codex is not a harness Ox offers; it offers claude, pi`,
  `huge is not a claude model Ox offers; it offers fast, smart`,
  `fast needs an effort; Ox offers low, medium, high`,
  `max is not an effort Ox offers for fast; it offers low, medium, high`,
  `low is not an effort Ox offers for smart; it has no effort levels`,
  `claude needs a mode; Ox offers ask, code`,
  `plan is not a claude mode Ox offers; it offers ask, code`,
  `ask is not a pi mode Ox offers; it has no modes`.
- A config without a `harnesses` key, a harness entry that names no harness,
  and unknown fields in an entry, model, or choice are errors naming the file.
- When `agents` fails (no or bad config), the UI drops to no harnesses as well
  as showing the error, so New Session offers nothing.
- `sessionOptions` leaves out a select with nothing listed (an unlisted
  harness, a model with no listed levels, a harness with no listed modes) and
  passes options of other categories through unchanged.
- A new session with no first prompt logs refused settings, as before.
- The e2e fixture's config lists the fake agent's starting values (`smart`,
  `medium`, `ask`), because a draft now sends every setting and the fake agent
  refuses values outside its own list.

## Checks run

- `make check-rust` — passed.
- `make check-ui` — passed (82 tests).
- `make check-scripts` — passed.
- `make e2e` — passed (2 spec files).
- `make smoke-claude` — passed.
- `make smoke-pi` — passed, including `effort_set_on_running_pi`.

## Manual verification

None beyond the checks. The request panel's mode name takes the effort name's
rule; it was not compared on screen in mock mode.

```sh
cd ui && npm run mock   # then open localhost:1430/mock/?scenario=sidebar
```

## Follow-up work

- `oxd-fake-agent`'s `--model-providers` flag has no remaining user.
