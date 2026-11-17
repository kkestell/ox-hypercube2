# Harnesses, models, and effort levels from a config file

## Problem

An agent that asks the user to start a session cannot find out which harnesses, models, and effort levels it may use, and it may leave model and effort out. The request then cannot say which model the session will run, and the user sees a block of "Harness: … Model: Default" text to approve. Ox learns what each harness offers by asking the harness, and the answer is mostly noise: Pi offers about 420 models, most of them cheap OpenRouter models with no place in a coding harness. The user works with a short list: Opus, Sonnet, and a little Haiku in Claude at their best-value effort levels; Luna, Terra, and Sol; a couple of OpenCode Go models; and a couple of OpenRouter models.

## Outcome

- Ox has a config file listing the harnesses, models, effort levels, and modes the user wants available in Ox. Each model lists its own effort levels, and each harness lists its modes.
- The model and effort picker in the app looks and works as it does today, and offers only what the config lists, in drafts and in running sessions, including running Pi sessions that report Pi's full model list.
- A draft shows a Mode picker offering the modes the config lists for its harness, and it starts on the mode last used, as the other pickers do.
- An agent can list the harnesses, models, and effort levels; the listing prints what the config lists.
- An agent's request to start a session names the harness, the model, and the effort level. Ox refuses a combination the config does not list and says what it does list.
- The request the user approves shows the harness logo with the model and effort as the composer shows them, the worktree or checkout as a session's header shows it, and the first prompt as plain text, with no warning-coloured border. This is the panel already built in this worktree's uncommitted UI changes.
- The repository holds an example config that matches the user's own, and `make install` copies it into Ox's config directory when no config is there yet, so installing on a new computer sets the config up. On a computer that already has a config, installing leaves it alone.
- Ox no longer asks harnesses which models, effort levels, and modes they offer.

## Decisions

- **The user lists models and effort levels in a config file; Ox does not ask the harnesses.** A short, chosen list is what the user wants to pick from, and it removes the work of asking each harness. Turned down: a store kept by the daemon and filled by asking each harness in the background, with each model's effort levels worked out from each harness, which was too complex; and an agent listing tool built on the harnesses' own lists, which returned about 30,000 characters for Pi.
- **The config lists each harness's modes, and a draft offers them.** The user picks a session's mode before its first prompt, and a draft keeps the mode last used rather than starting every Claude session in Manual. Turned down: drafts with no Mode picker, which start every Claude session in Manual; a list of modes built into Ox; and asking the harness for its modes.
- **Harness, model, and effort are required in a session request, and are checked against the config.** The agent chooses deliberately, and the user approves the exact model and effort the session starts with. Turned down: defaulting to the harness's own model and effort, which leaves the request unable to say which model the session runs.
- **The example config in the repository is the user's config.** `make install` copies it into place when no config exists yet, because the user moves between a work and a personal computer and `make install` is the only supported way to install Ox. It never overwrites an existing config.
- **A model or mode the harness does not accept fails when a session starts on it,** not when Ox reads the config.
- **Slash-command completion in a draft may go away** if it depends on asking the harness before the session starts.

## Out of scope

- What Ox does when it starts with no config file. The plan takes whatever is simplest.

## Left to the plan

- Where the config file lives and its format.
- The exact model and mode identifiers in the example config.
- How each harness accepts a model it no longer lists itself.
