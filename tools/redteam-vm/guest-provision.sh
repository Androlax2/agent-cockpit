#!/usr/bin/env bash
# Runs inside the guest as root, once, from "redteam-vm.sh create". Plain bash, sent as one
# argument: every command that could read stdin is given /dev/null.
set -euo pipefail

# What a Torlek session needs: tmux 3.6 or newer, git and curl for the install and the
# agents' work, bubblewrap for the sandbox.
pacman -Syu --noconfirm --needed tmux git curl bubblewrap </dev/null

# A user systemd session without a login, so "torlek daemon install" works over ssh.
loginctl enable-linger redteam </dev/null
