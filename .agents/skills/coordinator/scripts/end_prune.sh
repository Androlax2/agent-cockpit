#!/bin/sh
# Ends worker sessions, then prunes the target/ of clones nobody works in.
# Usage: end_prune.sh CLONES_ROOT SESSION... -- CLONE_DIR_NAME...
# Run it through `torlek ask run`: `torlek session end` is the owner's verb.
set -eu

if [ "$#" -lt 2 ] || [ ! -d "$1" ]; then
    echo "usage: end_prune.sh CLONES_ROOT SESSION... -- CLONE_DIR_NAME..." >&2
    exit 2
fi
# /proc/PID/cwd is always a resolved path, so compare against the resolved root.
root=$(cd "$1" && pwd -P)
shift
failed=0

while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do
    torlek session end "$1" || {
        echo "failed to end session $1" >&2
        failed=1
    }
    shift
done
[ "${1:-}" = "--" ] && shift

# A clone's session may still be closing its terminal; give it a moment before looking.
sleep "${END_PRUNE_SETTLE_SECONDS:-3}"

for name in "$@"; do
    case "$name" in
    "" | . | .. | */*)
        echo "refusing clone name: $name" >&2
        exit 2
        ;;
    esac
    clone="$root/$name"
    [ -d "$clone/target" ] || {
        echo "skip $name: no target/"
        continue
    }
    busy=0
    for cwd in /proc/[0-9]*/cwd; do
        case "$(readlink "$cwd" 2>/dev/null)" in
        "$clone" | "$clone"/*)
            busy=1
            break
            ;;
        esac
    done
    if [ "$busy" = 1 ]; then
        echo "skip $name: a process works in it"
        continue
    fi
    rm -rf -- "$clone/target"
    echo "pruned $name/target"
done
df -h "$root" | tail -1
exit "$failed"
