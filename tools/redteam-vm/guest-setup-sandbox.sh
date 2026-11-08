#!/usr/bin/env bash
# Runs inside the guest as the throwaway "redteam" user, from "redteam-vm.sh create", after
# Torlek is installed. It makes the guest ready for a section-1 sandboxed red-team session
# (issue #1158): the sandbox is required, and there is a trusted project to run the session in.
# None of this needs a provider credential; the owner drops a spend-capped key only at run time.
# Sent as one argument: commands that could read stdin are given /dev/null.
set -euo pipefail

export PATH=$HOME/.local/bin:$PATH

project=$HOME/redteam-project

# Enforce-or-refuse sandboxing for every agent session, and trust the red-team project so a
# section-1 session does not stop on the provider's own folder-trust prompt. There is no CLI
# verb to set these; the owner's config file is the one authority (crates/cockpit-config). The
# daemon reads it at start, so the clean snapshot carries it into every later boot.
mkdir -p ~/.config/torlek
cat >~/.config/torlek/config.toml <<TOML
# Written by tools/redteam-vm/guest-setup-sandbox.sh for the #1158 red-team harness.
[agents]
sandbox = "required"

[projects]
trusted = ["$project"]
TOML

# A plain git repository at its own root: "torlek project add" requires that (it refuses a
# subdirectory, a worktree or a separate git dir). A session needs a checkout to run in.
mkdir -p "$project"
git -C "$project" init -q </dev/null
git -C "$project" config user.email redteam@example.invalid </dev/null
git -C "$project" config user.name redteam </dev/null
: >"$project/README.md"
git -C "$project" add -A </dev/null
git -C "$project" commit -qm "red-team project" </dev/null

torlek project add "$project" </dev/null

# Apply the config to the daemon started by the install step, so a "check" before the first
# reset already sees the required sandbox.
torlek config reload </dev/null
