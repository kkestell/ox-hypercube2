.DEFAULT_GOAL := help
.PHONY: help dev install check check-rust check-ui check-scripts fix e2e smoke-claude smoke-pi

PYTHON ?= python3
RUFF ?= uvx ruff@0.17.4
CLAUDE_MODEL ?= claude-haiku-5-5
PI_MODEL ?= openrouter/deepseek/deepseek-v4.1-flash-1017
DEV_SOCKET := $(CURDIR)/target/dev/oxd.sock
APP_BUNDLE := $(CURDIR)/target/release/bundle/macos/Ox.app

help:
	@printf '%s\n' 'make dev             Run the app with its own oxd' 'make install         Build and install a locally signed Ox.app to ~/Applications' 'make check           Check formatting, lints, and tests' 'make check-rust      Check the Rust workspace' 'make check-ui        Check the UI' 'make check-scripts   Check scripts/' 'make fix             Apply formatting and safe lint fixes' 'make e2e             Run the app end-to-end tests' 'make smoke-claude    Run live Claude ACP smoke checks' 'make smoke-pi        Run live Pi ACP smoke checks'

DEV_OXD := pkill -f "[o]xd --socket $(DEV_SOCKET)$$"

# The dev oxd is stopped before building and whenever the app exits,
# including on Ctrl-C.
dev:
	-$(DEV_OXD)
	cargo build -p oxd -p ox-mcp -p claude-code-acp -p pi-acp
	trap '$(DEV_OXD)' EXIT; cd ui && OX_SOCKET="$(DEV_SOCKET)" npm run tauri dev

# Sign after adding the helpers, which are located beside the app executable.
install: export CARGO_TARGET_DIR := $(CURDIR)/target
install:
	@test "$$(uname -s)" = Darwin || { echo 'make install requires macOS' >&2; exit 1; }
	cargo build --release -p oxd -p ox-mcp -p claude-code-acp -p pi-acp
	cd ui && npm install && npm run tauri -- build --bundles app --no-sign
	cp target/release/oxd target/release/ox-mcp target/release/claude-code-acp target/release/pi-acp "$(APP_BUNDLE)/Contents/MacOS/"
	for binary in oxd ox-mcp claude-code-acp pi-acp; do \
		codesign --force --sign - "$(APP_BUNDLE)/Contents/MacOS/$$binary" || exit; \
	done
	codesign --force --sign - "$(APP_BUNDLE)"
	codesign --verify --deep --strict "$(APP_BUNDLE)"
	mkdir -p "$(HOME)/Applications"
	rm -rf "$(HOME)/Applications/Ox.app"
	ditto "$(APP_BUNDLE)" "$(HOME)/Applications/Ox.app"
	@printf '%s\n' "Installed $(HOME)/Applications/Ox.app"
	@config_home="$${XDG_CONFIG_HOME:-}"; \
	case "$$config_home" in /*) ;; *) config_home="$(HOME)/.config";; esac; \
	config="$$config_home/ox/config.json"; \
	if [ ! -e "$$config" ]; then \
		mkdir -p "$$config_home/ox" && cp config/config.json "$$config" && printf '%s\n' "Installed $$config"; \
	fi

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
