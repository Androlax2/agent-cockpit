SHELL := /bin/bash
.SHELLFLAGS := -euo pipefail -c
.DEFAULT_GOAL := help

# Agent Cockpit lives in ~/agent-cockpit, the running copy: its files are linked into ~/.local/bin,
# ~/.config and ~/.claude by stow, one link per file (--no-folding), so the dotfiles' packages of the
# same names (config/<package> there) can link their own files into the same folders.
stow = cd config && stow -v --no-folding -t ~
packages = $(shell ls config)
build_runner = python3 config/bin/.local/bin/agent-build
# One install at a time: overlapping runs share migrate-cockpit-names.py's service snapshot, and the
# first run's --services deleted it under the other (2026-10-27). Usage: $(call serialized,<target>).
install_lock = $${XDG_RUNTIME_DIR:-/run/user/$$(id -u)}/cockpit-install.lock
serialized = flock -n "$(install_lock)" true || echo "Waiting for another install or apply to finish …"; \
	flock "$(install_lock)" $(MAKE) --no-print-directory $(1)

.PHONY: help build test test-serialized install install-serialized system after-apply after-apply-serialized handoff-install greywall-patch greyproxy-patch

help:
	@use_color=0; if [ -t 1 ] && [ -z "$${NO_COLOR:-}" ]; then use_color=1; fi; \
	awk -v use_color="$$use_color" -f tools/make-help.awk $(MAKEFILE_LIST)

##@ Build and install

build: ## Build the Rust programs in apps/ (the column, the osd daemon, the approval terminal, the guide, the sandbox commands)
	$(build_runner) cargo build --release --locked --manifest-path apps/Cargo.toml

# One full run at a time across clones (tools/full-test-lock.py); targeted runs stay unlocked. The
# launcher shares this folder with every sandbox.
test: ## Build the OSD classifier and run the test suite, one full run at a time across clones
	@python3 tools/full-test-lock.py "$(HOME)/.cache/cockpit/test-lock" "$(CURDIR)" $(MAKE) --no-print-directory test-serialized

test-serialized:
	python3 -m unittest discover -s tools/tests -p 'test_*.py'
	$(build_runner) cargo build --release --locked --quiet --manifest-path apps/Cargo.toml -p osd --bin osd
	$(build_runner) cargo nextest run --release --locked --cargo-quiet --status-level fail --manifest-path apps/Cargo.toml

.PHONY: fmt-check clippy doc

fmt-check: ## Check Rust formatting without changing source
	$(build_runner) cargo fmt --manifest-path apps/Cargo.toml --all -- --check

clippy: ## Lint every Rust target with warnings treated as errors
	$(build_runner) cargo clippy --manifest-path apps/Cargo.toml --workspace --all-targets --locked -- -D warnings

doc: ## Check private and public Rust documentation
	RUSTDOCFLAGS="-D warnings" $(build_runner) cargo doc --manifest-path apps/Cargo.toml --workspace --no-deps --document-private-items --locked

.PHONY: deny

deny: ## Check dependency advisories, licenses and sources with the reviewed policy
	$(build_runner) cargo deny --manifest-path apps/Cargo.toml --locked --config .cargo/deny.toml check

# The greenfield workspace (root Cargo.toml and crates/) grows beside apps/ until the M5 cutover
# (docs/design/greenfield/20-refactoring-plan.md); each check above has its counterpart here.
.PHONY: crates-build crates-test crates-fmt-check crates-clippy crates-doc crates-deny

crates-build: ## Build the greenfield workspace in crates/ (debug profile, into target/)
	$(build_runner) cargo build --locked --manifest-path Cargo.toml --workspace --all-targets

crates-test: ## Run the greenfield workspace's tests
	$(build_runner) cargo nextest run --locked --cargo-quiet --status-level fail --manifest-path Cargo.toml --workspace

crates-fmt-check: ## Check the greenfield workspace's formatting without changing source
	$(build_runner) cargo fmt --manifest-path Cargo.toml --all -- --check

crates-clippy: ## Lint every greenfield target with warnings treated as errors
	$(build_runner) cargo clippy --manifest-path Cargo.toml --workspace --all-targets --locked -- -D warnings

crates-doc: ## Check the greenfield workspace's private and public documentation
	RUSTDOCFLAGS="-D warnings" $(build_runner) cargo doc --manifest-path Cargo.toml --workspace --no-deps --document-private-items --locked

crates-deny: ## Check the greenfield workspace's dependencies with the same reviewed policy
	$(build_runner) cargo deny --manifest-path Cargo.toml --locked --config .cargo/deny.toml check

.PHONY: sandbox-install-smoke

sandbox-install-smoke: ## Verify pinned engine installation and doctor in a fresh temporary home
	$(build_runner) cargo build --locked -j 2 -p cockpit-cli -p cockpit-daemon
	python3 tools/verify-engine-install.py

greenfield-ci: crates-fmt-check crates-clippy crates-doc crates-deny typos crates-test sandbox-install-smoke ## Run the greenfield workspace's gate (CI's greenfield job)

.PHONY: lint-python lint-shell lint-js lint-markdown lint-lua

lint-python: ## Check Python style and formatting, including extensionless commands
	python3 tools/quality.py python

lint-shell: ## Check POSIX/Bash scripts and fish with their own tools
	python3 tools/quality.py shell
	python3 tools/quality.py fish

lint-js: ## Check JavaScript and TypeScript
	python3 tools/quality.py js

lint-markdown: ## Check Markdown style
	python3 tools/quality.py markdown

.PHONY: check-links

check-links: ## Check local Markdown links and heading anchors without network requests
	python3 tools/quality.py links

.PHONY: typos

typos: ## Check spelling in tracked files
	python3 tools/quality.py typos

.PHONY: ci greenfield-ci docs-ci
.NOTPARALLEL: ci greenfield-ci docs-ci

docs-ci: lint-markdown check-links typos ## Check Markdown style, local links and spelling (CI's docs job)

ci: fmt-check clippy doc lint-python lint-shell lint-js lint-markdown lint-lua check-links typos deny test ## Run the quality and regression gate for apps/, config/ and tools/ (CI's legacy job)

lint-lua: ## Check Lua syntax without executing desktop configuration
	python3 tools/quality.py lua

install: ## Link every package into your home and reload the user units
	@$(call serialized,install-serialized)

install-serialized: build
	python3 tools/migrate-cockpit-names.py --prepare
	$(stow) --restow $(packages)
	python3 tools/migrate-cockpit-names.py --apply
	systemctl --user daemon-reload
	python3 tools/migrate-cockpit-names.py --services

system: ## [sudo] Copy system/ into /: each changed file shows its diff and asks first
	@tools/system-install.sh

# cockpit apply runs this in the running copy after git am, with BEFORE the commit it started from.
# A new greywall or greyproxy pin is built and installed too: sessions are flagged for a restart once
# greywall changes (the column watches it), greyproxy restarts on its new build. The last lines fail
# loud whenever either installed binary is not the pinned build, whatever changed, or greyproxy's
# conversation tracking (a core of CPU with busy sessions) cannot be kept off.
after-apply: ## Relink, rebuild and reload when apps/ changed, rebuild greywall or greyproxy when its pin did: make after-apply BEFORE=<commit>
	@test -n "$(BEFORE)" || { echo "usage: make after-apply BEFORE=<commit before the apply>"; exit 2; }
	@$(call serialized,after-apply-serialized BEFORE="$(BEFORE)")

after-apply-serialized:
	python3 tools/migrate-cockpit-names.py --prepare
	@if ! git diff --quiet "$(BEFORE)" HEAD -- apps; then $(MAKE) build; fi
	$(stow) --restow $(packages)
	python3 tools/migrate-cockpit-names.py --apply
	systemctl --user daemon-reload
	python3 tools/migrate-cockpit-names.py --services
	@if ! git diff --quiet "$(BEFORE)" HEAD -- apps; then cockpit reload; fi
	systemctl --user start cockpit-failsafe.timer
	@if ! git diff --quiet "$(BEFORE)" HEAD -- packages/greywall/build.sh; then $(MAKE) greywall-patch; fi
	@if ! git diff --quiet "$(BEFORE)" HEAD -- packages/greyproxy/build.sh; then $(MAKE) greyproxy-patch; fi
	@packages/greywall/check.sh
	@packages/greyproxy/check.sh
	@packages/greyproxy/settings.sh

handoff-install: ## Install what an agent left in .handoff/ (checksums, diff, then asks; runs with your rights)
	@tools/handoff-install.sh

##@ The sandbox stack

# greywall from the dotfiles branch of Androlax2/greywall: denyRead masks applied last, so a sandbox
# cannot read greyproxy's CA key, its session key or ~/.ssh/id_*. Restart sessions afterwards.
greywall-patch: ## Build, test and install the patched greywall into ~/.local/bin (keeps greywall.previous)
	packages/greywall/build.sh
	@packages/greywall/check.sh

# greyproxy from the dotfiles branch of Androlax2/greyproxy: v0.4.5 plus the self-listener block,
# an optional API token, per-client proxy secrets (both off) and a fail-closed start.
greyproxy-patch: ## Build, test, install and restart greyproxy from the fork (keeps greyproxy.previous)
	packages/greyproxy/build.sh
	@packages/greyproxy/check.sh

##@ Isolate coding agents

agent_docker = tools/agent-docker.sh

.PHONY: agent-docker-setup agent-docker-add agent-docker-remove agent-docker-grant agent-docker-status

# One rootless Docker daemon per project for agents running in greywall (tools/agent-docker.sh).
# The units and the agent_docker_egress firewall table come from system/.
agent-docker-setup: system ## [sudo] Install the agent units and the agent-docker group (log out and back in after the first run)
	sudo $(agent_docker) setup

agent-docker-add: ## [sudo] Give a project its own isolated Docker daemon: make agent-docker-add PROJECT=runway
	@test -n "$(PROJECT)" || { echo "usage: make agent-docker-add PROJECT=<folder in ~/IdeaProjects>"; exit 2; }
	sudo $(agent_docker) add "$(PROJECT)"

agent-docker-remove: ## [no undo] Delete a project's agent daemon, its user, images and containers: make agent-docker-remove PROJECT=runway
	@test -n "$(PROJECT)" || { echo "usage: make agent-docker-remove PROJECT=<folder in ~/IdeaProjects>"; exit 2; }
	sudo $(agent_docker) remove "$(PROJECT)"

agent-docker-grant: ## [sudo] Let a non-root container user write named folders: make agent-docker-grant PROJECT=stadeo CONTAINER_UID=1000 FOLDERS="storage bootstrap/cache"
	@test -n "$(PROJECT)" -a -n "$(CONTAINER_UID)" -a -n "$(FOLDERS)" || { echo 'usage: make agent-docker-grant PROJECT=<folder in ~/IdeaProjects> CONTAINER_UID=<uid> FOLDERS="<folder>..."'; exit 2; }
	sudo $(agent_docker) grant "$(PROJECT)" "$(CONTAINER_UID)" $(FOLDERS)

agent-docker-status: ## [read-only] List the agent Docker daemons and whether their sockets are up
	@$(agent_docker) status

agent_browser = $(HOME)/.cache/agent-browser
playwright_version = 1.64.1

.PHONY: agent-browser agent-forward

# ~/.cache is read-only inside every sandbox, so sessions can use this browser but not alter it.
agent-browser: ## Give every sandbox headless Chromium through Playwright for Python: ~/.cache/agent-browser/bin/python
	uv venv --allow-existing --python /usr/bin/python3 $(agent_browser)
	uv pip install --python $(agent_browser)/bin/python playwright==$(playwright_version)
	$(agent_browser)/bin/playwright install chromium

agent-forward: ## Forward host ports into a project's sandboxes (new sessions): make agent-forward PROJECT=stadeo PORTS="8088" DAYS=30
	@test -n "$(PROJECT)" -a -n "$(PORTS)" || { echo 'usage: make agent-forward PROJECT=<folder in ~/IdeaProjects> PORTS="<port>..." [DAYS=30]'; exit 2; }
	@tools/agent-forward.sh "$(PROJECT)" "$(or $(DAYS),30)" $(PORTS)

.PHONY: agent-anthropic-key agent-opencode-key

agent-anthropic-key: ## Store the Console API key that credits sessions (the cockpit's $) bill instead of the subscription; asks for it
	@secret-tool store --label "Claude API key for credits sessions" service agent-anthropic

agent-opencode-key: ## Store the Anthropic API key opencode sessions use, a greyproxy placeholder inside their sandbox; asks for it
	@read -rsp "Anthropic API key for opencode (sk-ant-…, not shown): " key; echo; \
	if [[ ! $$key =~ ^sk-ant-[a-z]+[0-9]*-[A-Za-z0-9_-]+$$ ]] || (( $${#key} < 80 )); then \
	    echo "not stored: an Anthropic API key starts with sk-ant- and has 80 characters or more (this one: $${#key})" >&2; exit 1; \
	fi; \
	printf '%s' "$$key" | secret-tool store --label "Anthropic API key for opencode sessions" service agent-opencode-anthropic; \
	echo "stored ($${#key} characters)"

agent_github = tools/agent-github.sh
ALLOW_WORKFLOWS ?= 0

.PHONY: agent-github-token agent-github-board-token agent-github-relogin agent-github-protect agent-github-check \
	    agent-dashboard-tailnet-grant agent-dashboard-tailnet agent-dashboard-tailnet-off

agent-github-token: ## Create a repo-only token without workflow write access (opt in: ALLOW_WORKFLOWS=1)
	@test -n "$(PROJECT)" || { echo "usage: make agent-github-token PROJECT=<folder in ~/IdeaProjects>"; exit 2; }
	@ALLOW_WORKFLOWS="$(ALLOW_WORKFLOWS)" $(agent_github) token "$(PROJECT)"

agent-github-board-token: ## Create and store the classic token, project scope only, that lets agents read and move Project board cards
	@$(agent_github) project-board-token

agent-github-relogin: ## Revoke your old GitHub CLI login and log in again with keyring storage
	@$(agent_github) relogin

agent-github-protect: ## Require pull requests and block force pushes on the project's default branch: make agent-github-protect PROJECT=runway
	@test -n "$(PROJECT)" || { echo "usage: make agent-github-protect PROJECT=<folder in ~/IdeaProjects>"; exit 2; }
	@$(agent_github) protect "$(PROJECT)"

agent-github-check: ## [read-only] Check that each agent token reaches only its repo and that gh keeps no token in hosts.yml
	@$(agent_github) check

# greyproxy's dashboard listens on localhost only; `tailscale serve` publishes it to this tailnet
# (and nowhere else) so pending network requests can be approved from the phone. It has no login:
# every device on the tailnet would reach it. So it gets a port of its own, which a tailnet grant
# keeps to the phone (agent-dashboard-tailnet-grant prints it), and publishing waits until Theo says
# the grant is in place (GRANTED=yes). Port 443 stays the phone page's (claude-remote). tailscaled
# takes serve changes from root only (no operator is set).
GREYPROXY_DASHBOARD_PORT ?= 43080
DASHBOARD_TAILNET_PORT ?= 8443
agent-dashboard-tailnet-grant: ## [read-only] Print the tailnet grants that keep the dashboard to one phone: make agent-dashboard-tailnet-grant PHONE=iphone
	@test -n "$(PHONE)" || { echo "usage: make agent-dashboard-tailnet-grant PHONE=<phone name, as tailscale status shows it>"; exit 2; }
	@tools/tailnet-dashboard-grant.sh "$(PHONE)" "$(DASHBOARD_TAILNET_PORT)"

agent-dashboard-tailnet: ## [sudo] Publish greyproxy's dashboard to the phone, once its grant is in place: make agent-dashboard-tailnet GRANTED=yes (then open :8443/dashboard)
	@test "$(GRANTED)" = yes || { echo "Without a grant every device on the tailnet reaches the dashboard, with no login."; \
	    echo "Paste make agent-dashboard-tailnet-grant PHONE=<phone> into the tailnet's access controls first, then: make agent-dashboard-tailnet GRANTED=yes"; exit 2; }
	sudo tailscale serve --bg --https=$(DASHBOARD_TAILNET_PORT) http://127.0.0.1:$(GREYPROXY_DASHBOARD_PORT)
	tailscale serve status

agent-dashboard-tailnet-off: ## [sudo] Stop publishing greyproxy's dashboard on the tailnet
	sudo tailscale serve --https=$(DASHBOARD_TAILNET_PORT) off
