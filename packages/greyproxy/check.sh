#!/usr/bin/env bash
# Whether the installed greyproxy is the build of the commit build.sh pins: prints both commits, and
# exits 1 when they differ or the binary does not say (make after-apply, after greyproxy-patch).
#   packages/greyproxy/check.sh [greyproxy binary, default ~/.local/bin/greyproxy]
set -euo pipefail

binary=${1:-$HOME/.local/bin/greyproxy}
pinned=$(sed -n 's/^greyproxy_commit=\([0-9a-f]\{40\}\).*/\1/p' "$(dirname "$0")/build.sh")
[[ -n $pinned ]] || { echo "no greyproxy_commit pin in $(dirname "$0")/build.sh" >&2; exit 2; }
version=$("$binary" --version 2>/dev/null) || true
installed=$(sed -n 's/^ *commit: *\([0-9a-f]\{40\}\).*/\1/p' <<< "$version")
if [[ $installed == "$pinned" ]]; then
    echo "greyproxy is the pinned build (${pinned:0:7})"
    exit 0
fi
echo "greyproxy is ${installed:-a build that names no commit}, the pin is $pinned: make -C ~/agent-cockpit greyproxy-patch" >&2
exit 1
