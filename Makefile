SHELL := /bin/bash
.SHELLFLAGS := -euo pipefail -c
.DEFAULT_GOAL := help

# Agent Cockpit lives in ~/agent-cockpit, the running copy: its files are linked into ~/.local/bin,
# ~/.config and ~/.claude by stow, one link per file (--no-folding), so the dotfiles' packages of the
# same names (config/<package> there) can link their own files into the same folders.
stow = cd config && stow -v --no-folding -t ~
packages = $(shell ls config)
build_runner = python3 config/bin/.local/bin/agent-build

.PHONY: help build test install system after-apply handoff-install greywall-patch greyproxy-patch

help:
	@use_color=0; if [ -t 1 ] && [ -z "$${NO_COLOR:-}" ]; then use_color=1; fi; \
	awk -v use_color="$$use_color" -f tools/make-help.awk $(MAKEFILE_LIST)

##@ Build and install

build: ## Build the Rust programs in apps/ (the column, the osd daemon, the approval terminal, the guide, the sandbox commands)
	$(build_runner) cargo build --release --manifest-path apps/Cargo.toml

test: ## Build the OSD classifier and run the test suite
	python3 -m unittest discover -s tools/tests -p 'test_*.py'
	$(build_runner) cargo build --release --quiet --manifest-path apps/Cargo.toml -p osd --bin osd
	$(build_runner) cargo test --release --quiet --manifest-path apps/Cargo.toml

install: build ## Link every package into your home and reload the user units
	$(stow) --restow $(packages)
	systemctl --user daemon-reload

system: ## [sudo] Copy system/ into /: each changed file shows its diff and asks first
	@tools/system-install.sh

# cockpit apply runs this in the running copy after git am, with BEFORE the commit it started from.
after-apply: ## Relink, and rebuild and reload when apps/ changed: make after-apply BEFORE=<commit>
	@test -n "$(BEFORE)" || { echo "usage: make after-apply BEFORE=<commit before the apply>"; exit 2; }
	$(stow) --restow $(packages)
	systemctl --user daemon-reload
	@if ! git diff --quiet "$(BEFORE)" HEAD -- apps; then $(MAKE) build && cockpit reload; fi

handoff-install: ## Install what an agent left in .handoff/ (checksums, diff, then asks; runs with your rights)
	@tools/handoff-install.sh

##@ The sandbox stack

# greywall from the dotfiles branch of Androlax2/greywall: denyRead masks applied last, so a sandbox
# cannot read greyproxy's CA key, its session key or ~/.ssh/id_*. Restart sessions afterwards.
greywall-patch: ## Build, test and install the patched greywall into ~/.local/bin (keeps greywall.previous)
	packages/greywall/build.sh

# greyproxy from the dotfiles branch of Androlax2/greyproxy: v0.4.5 plus the self-listener block,
# an optional API token, per-client proxy secrets (both off) and a fail-closed start.
greyproxy-patch: ## Build, test and install greyproxy from the fork into ~/.local/bin (keeps greyproxy.previous)
	packages/greyproxy/build.sh

##@ Isolate coding agents

agent_docker = tools/agent-docker.sh

.PHONY: agent-docker-setup agent-docker-add agent-docker-remove agent-docker-status

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

agent-docker-status: ## [read-only] List the agent Docker daemons and whether their sockets are up
	@$(agent_docker) status

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

.PHONY: agent-github-token agent-github-board-token agent-github-relogin agent-github-protect agent-github-check \
	    agent-dashboard-tailnet agent-dashboard-tailnet-off

agent-github-token: ## Create and store a GitHub token that reaches only this project's repo: make agent-github-token PROJECT=runway
	@test -n "$(PROJECT)" || { echo "usage: make agent-github-token PROJECT=<folder in ~/IdeaProjects>"; exit 2; }
	@$(agent_github) token "$(PROJECT)"

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
# (and nowhere else) so pending network requests can be approved from the phone.
GREYPROXY_DASHBOARD_PORT ?= 43080
agent-dashboard-tailnet: ## Publish greyproxy's dashboard on the tailnet: make agent-dashboard-tailnet [GREYPROXY_DASHBOARD_PORT=43080] (then open /dashboard)
	tailscale serve --bg --https=443 http://127.0.0.1:$(GREYPROXY_DASHBOARD_PORT)
	tailscale serve status

agent-dashboard-tailnet-off: ## Stop publishing greyproxy's dashboard on the tailnet
	tailscale serve --https=443 off
