.DEFAULT_GOAL := help
.PHONY: help dev install install-ui check check-rust check-ui check-scripts fix e2e smoke-claude smoke-pi

PYTHON ?= python3
RUFF ?= uvx ruff@0.17.4
CLAUDE_MODEL ?= claude-haiku-5-5
PI_MODEL ?= openrouter/deepseek/deepseek-v4.1-flash-1017
DEV_SOCKET := $(CURDIR)/target/dev/oxd.sock
APP_BUNDLE := $(CURDIR)/target/release/bundle/macos/Ox.app

help:
	@printf '%s\n' 'make dev             Run the app with its own oxd' 'make install         Build and install a locally signed Ox.app to ~/Applications, restarting oxd' 'make install-ui      Reinstall only the app, keeping the running oxd' 'make check           Check formatting, lints, and tests' 'make check-rust      Check the Rust workspace' 'make check-ui        Check the UI' 'make check-scripts   Check scripts/' 'make fix             Apply formatting and safe lint fixes' 'make e2e             Run the app end-to-end tests' 'make smoke-claude    Run live Claude ACP smoke checks' 'make smoke-pi        Run live Pi ACP smoke checks'

DEV_OXD := pkill -f "[o]xd --socket $(DEV_SOCKET)$$"

# The dev oxd is stopped before building and whenever the app exits,
# including on Ctrl-C.
dev:
	-$(DEV_OXD)
	cargo build -p oxd -p ox-mcp -p claude-code-acp -p pi-acp
	trap '$(DEV_OXD)' EXIT; cd ui && OX_SOCKET="$(DEV_SOCKET)" npm run tauri dev

INSTALLED_APP := $(HOME)/Applications/Ox.app
HELPERS := oxd ox-mcp claude-code-acp pi-acp

# Quits the installed app, if it is running, and waits for it to exit,
# setting `was_running`. The app goes before oxd, since a running app starts
# oxd again from its bundle as soon as its connection closes. `pgrep -a`
# includes the app when it is an ancestor, as it is for an agent in an Ox
# session.
QUIT_OX := was_running=; \
	if pgrep -qaf '$(INSTALLED_APP)/Contents/MacOS/ox$$'; then \
		was_running=1; \
		osascript -e 'tell application "Ox" to quit' >/dev/null; \
		while pgrep -qaf '$(INSTALLED_APP)/Contents/MacOS/ox$$'; do sleep 0.2; done; \
	fi
REPLACE_OX := mkdir -p "$(dir $(INSTALLED_APP))" && \
	rm -rf "$(INSTALLED_APP)" && \
	ditto "$(APP_BUNDLE)" "$(INSTALLED_APP)" && \
	printf '%s\n' "Installed $(INSTALLED_APP)"
OPEN_OX := open "$(INSTALLED_APP)"

# Sign after adding the helpers, which are located beside the app executable.
# Nothing running is disturbed until the bundle is built. oxd stops only when
# no session is busy; otherwise the app is relaunched and the install fails.
install: export CARGO_TARGET_DIR := $(CURDIR)/target
install:
	@test "$$(uname -s)" = Darwin || { echo 'make install requires macOS' >&2; exit 1; }
	cargo build --release -p oxd -p ox-mcp -p claude-code-acp -p pi-acp
	cd ui && npm install && npm run tauri -- build --bundles app --no-sign
	cp target/release/oxd target/release/ox-mcp target/release/claude-code-acp target/release/pi-acp "$(APP_BUNDLE)/Contents/MacOS/"
	for binary in $(HELPERS); do \
		codesign --force --sign - "$(APP_BUNDLE)/Contents/MacOS/$$binary" || exit; \
	done
	codesign --force --sign - "$(APP_BUNDLE)"
	codesign --verify --deep --strict "$(APP_BUNDLE)"
	@$(QUIT_OX); \
	target/release/oxd --stop || { [ -z "$$was_running" ] || $(OPEN_OX); exit 1; }; \
	$(REPLACE_OX) || exit; \
	config_home="$${XDG_CONFIG_HOME:-}"; \
	case "$$config_home" in /*) ;; *) config_home="$(HOME)/.config";; esac; \
	config="$$config_home/ox/config.json"; \
	if [ ! -e "$$config" ]; then \
		mkdir -p "$$config_home/ox" && cp config/config.json "$$config" && printf '%s\n' "Installed $$config"; \
	fi; \
	$(OPEN_OX)

# Rebuilds only the app and keeps the installed helpers, signatures and all,
# so the running oxd, and any oxd the app starts later, is the installed one.
install-ui: export CARGO_TARGET_DIR := $(CURDIR)/target
install-ui:
	@test -d "$(INSTALLED_APP)" || { echo 'Ox is not installed; run make install' >&2; exit 1; }
	cd ui && npm install && npm run tauri -- build --bundles app --no-sign
	for binary in $(HELPERS); do \
		cp -p "$(INSTALLED_APP)/Contents/MacOS/$$binary" "$(APP_BUNDLE)/Contents/MacOS/" || exit; \
	done
	codesign --force --sign - "$(APP_BUNDLE)"
	codesign --verify --deep --strict "$(APP_BUNDLE)"
	@$(QUIT_OX); $(REPLACE_OX) && $(OPEN_OX)

check: check-rust check-ui check-scripts

check-rust:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	cargo build -p ox-mcp
	cargo test --workspace

check-ui:
	cd ui && npm run check && npm test

check-scripts:
	$(RUFF) check scripts
	$(RUFF) format --check scripts

fix:
	cargo fmt --all
	cd ui && npm run fix
	$(RUFF) check --fix scripts
	$(RUFF) format scripts

# Build the app with the embedded WebDriver server and bundled UI.
e2e:
	cargo build -p oxd -p ox-mcp
	cd ui && npm run tauri -- build --debug --no-bundle --features webdriver
	cd ui && npm run e2e

smoke-claude:
	$(PYTHON) scripts/smoke-acp.py --agent claude --model "$(CLAUDE_MODEL)"

smoke-pi:
	$(PYTHON) scripts/smoke-acp.py --agent pi --model "$(PI_MODEL)"
