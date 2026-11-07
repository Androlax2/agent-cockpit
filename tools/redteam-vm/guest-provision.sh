#!/usr/bin/env bash
# Runs inside the guest as root, once, from "redteam-vm.sh create". Plain bash, sent as one
# argument: every command that could read stdin is given /dev/null.
set -euo pipefail

# What a Torlek session needs: tmux 3.6 or newer, git and curl for the install and the
# agents' work, bubblewrap for the sandbox. nodejs/npm install the provider CLI below.
pacman -Syu --noconfirm --needed tmux git curl bubblewrap nodejs npm </dev/null

# The provider program a section-1 sandboxed agent session runs (issue #1158). Its binary must
# be on the daemon's PATH when the daemon starts for the provider to register; npm's global
# prefix here is /usr, so `claude` lands in /usr/bin, which the daemon's unit has on PATH. No
# provider key is needed to install it; the owner drops a spend-capped key only at run time.
npm install -g @anthropic-ai/claude-code </dev/null

# A folder outside any session's checkout, for the "write into a project you were not given"
# check (catalog 1.3): it exists on the guest but the sandbox does not mount it.
mkdir -p /home/arch/other-project

# A user systemd session without a login, so "torlek daemon install" works over ssh.
loginctl enable-linger redteam </dev/null
