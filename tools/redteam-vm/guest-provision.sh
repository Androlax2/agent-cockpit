#!/usr/bin/env bash
# Runs inside the guest as root, once, from "redteam-vm.sh create". Plain bash, sent as one
# argument: every command that could read stdin is given /dev/null.
set -euo pipefail

# What a Torlek session needs: tmux 3.6 or newer, git and curl for the install and the
# agents' work, bubblewrap for the sandbox. The provider program is installed separately, at a
# pinned version, by guest-install-claude.sh (it needs neither node nor npm).
pacman -Syu --noconfirm --needed tmux git curl bubblewrap </dev/null

# A folder outside any session's checkout, for the "write into a project you were not given"
# check (catalog 1.3): it exists on the guest but the sandbox does not mount it.
mkdir -p /home/arch/other-project

# A user systemd session without a login, so "torlek daemon install" works over ssh.
loginctl enable-linger redteam </dev/null
