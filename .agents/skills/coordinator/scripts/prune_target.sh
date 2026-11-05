#!/bin/sh
# Deletes the target/ folder of finished workers' clones that nobody works in.
# Usage: prune_target.sh CLONES_ROOT CLONE_DIR_NAME...
# Run it through `torlek ask run`: it deletes files outside your sandbox. End the worker first
# (`torlek agent end SESSION`).
set -eu

if [ "$#" -lt 2 ] || [ ! -d "$1" ]; then
    echo "usage: prune_target.sh CLONES_ROOT CLONE_DIR_NAME..." >&2
    exit 2
fi
# /proc/PID/cwd is always a resolved path, so compare against the resolved root.
root=$(cd "$1" && pwd -P)
shift

for name in "$@"; do
    case "$name" in
    "" | . | .. | */*)
        echo "refusing clone name: $name" >&2
        exit 2
        ;;
    esac
    clone="$root/$name"
    # The busy check below compares resolved /proc paths, so a symlinked clone could not be
    # matched, and rm would work through the link.
    if [ -L "$clone" ]; then
        echo "refusing symlinked clone: $name" >&2
        exit 2
    fi
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
