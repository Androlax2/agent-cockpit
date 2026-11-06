#!/bin/sh
# Runs inside the declared WSL 2 distribution, started by torlek-wsl.ps1 through
# `wsl.exe --cd <Scoop app directory> --exec /bin/sh ./install-in-wsl.sh` (docs/packaging/windows.md).
#
# Usage: install-in-wsl.sh check TAG
#        install-in-wsl.sh install TAG
#
# The working directory holds the release's SHA256SUMS, its linux-x86_64 archive (saved by Scoop
# under a name without an archive extension so that Scoop does not unpack it on NTFS) and
# install.sh, the one installer of every archive route. This script adds only what is particular
# to WSL, then runs install.sh on those files: `check` verifies and changes nothing, `install`
# unpacks into ~/.local/lib/torlek/torlek-vTAG-linux-x86_64 and links ~/.local/bin/torlek to it.
set -eu

fail() {
    printf 'torlek-wsl: %s\n' "$1" >&2
    exit 1
}

[ "$#" -eq 2 ] || fail "usage: install-in-wsl.sh check|install TAG"
action=$1
tag=$2
case "$action" in
check | install) ;;
*) fail "unknown action '$action': use check or install" ;;
esac

kernel=$(uname -r)
case "$kernel" in
*microsoft-standard-WSL2*) ;;
*) fail "kernel '$kernel' is not a WSL 2 kernel. Torlek needs WSL 2; converting a distribution is your decision: wsl.exe --set-version <distribution> 2" ;;
esac
machine=$(uname -m)
[ "$machine" = x86_64 ] ||
    fail "this distribution runs on $machine, but the Windows package carries only the linux-x86_64 archive. Install the linux-aarch64 archive inside WSL by hand (docs/packaging/windows.md)"
[ "$(id -u)" -ne 0 ] ||
    fail "running as root. Torlek installs per user: set the distribution's default user to your own account, then retry"

[ -f install.sh ] || fail "install.sh is missing from $(pwd); reinstall the Scoop package"
printf 'WSL 2 distribution (%s, %s); install.sh checks the rest.\n' "$machine" "$kernel"
check=
[ "$action" = install ] || check=--check
exec /bin/sh ./install.sh --version "v$tag" --archive "torlek-v$tag-linux-x86_64.archive" \
    --sums SHA256SUMS ${check:+"$check"}
