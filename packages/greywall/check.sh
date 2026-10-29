#!/usr/bin/env bash
# Whether the installed greywall is the build of the commit build.sh pins: prints both commits, and
# exits 1 when they differ or the binary does not say (make after-apply, cockpit doctor).
#   packages/greywall/check.sh [greywall binary, default ~/.local/bin/greywall]
set -euo pipefail

binary=${1:-$HOME/.local/bin/greywall}
pinned=$(sed -n 's/^greywall_commit=\([0-9a-f]\{40\}\).*/\1/p' "$(dirname "$0")/build.sh")
[[ -n $pinned ]] || {
    echo "no greywall_commit pin in $(dirname "$0")/build.sh" >&2
    exit 2
}
# Its exit status is not the point: --version also reports the system's capabilities.
version=$("$binary" --version 2>/dev/null) || true
installed=$(sed -n 's/^ *Commit: *\([0-9a-f]\{40\}\).*/\1/p' <<<"$version")
if [[ $installed == "$pinned" ]]; then
    echo "greywall is the pinned build (${pinned:0:7})"
    exit 0
fi
echo "greywall is ${installed:-a build that names no commit}, the pin is $pinned: make -C ~/agent-cockpit greywall-patch" >&2
exit 1
