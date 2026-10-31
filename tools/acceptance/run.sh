#!/usr/bin/env bash
# Runs the new Cockpit for a live acceptance walk (docs/acceptance/m1.md, #135): the daemon,
# CLI and TUI built in this clone's target/release, on scratch XDG directories and a private
# tmux directory, so the installed (legacy) cockpit and its state are never touched.
#
#   run.sh start              start the daemon, register the scratch project `demo`
#   run.sh tui                attach this terminal to the TUI
#   run.sh cockpit ARGS...    run this clone's `cockpit`, from the scratch projects directory
#   run.sh session VERB       sleep|wake|resume|end the newest such session of `demo` in place
#   run.sh kill-daemon        kill -9 the daemon, leaving its sessions running
#   run.sh stop               stop the daemon and the scratch tmux server
#   run.sh clean              stop, then delete the scratch directory
#
# The scratch directory is $COCKPIT_ACCEPTANCE_HOME, by default /tmp/cockpit-acceptance-<uid>:
# short, because the daemon's sockets live under it. HOME stays the owner's own, so the agent
# uses the owner's Claude Code login.
set -euo pipefail

usage="usage: run.sh start|tui|kill-daemon|stop|clean | cockpit ARGS... | session sleep|wake|resume|end"

clone=$(cd "$(dirname "$(readlink -f "$0")")/../.." && pwd)
bin="$clone/target/release"
scratch=${COCKPIT_ACCEPTANCE_HOME:-/tmp/cockpit-acceptance-$(id -u)}
[[ $scratch == /* ]] || {
    echo "COCKPIT_ACCEPTANCE_HOME must be an absolute path: $scratch" >&2
    exit 2
}
projects="$scratch/projects"
demo="$projects/demo"
pid_file="$scratch/daemon.pid"
daemon_output="$scratch/daemon.out"
# `clean` deletes only a directory this script made.
marker=.cockpit-acceptance

export XDG_CONFIG_HOME="$scratch/config"
export XDG_STATE_HOME="$scratch/state"
export XDG_CACHE_HOME="$scratch/cache"
export XDG_RUNTIME_DIR="$scratch/runtime"
# The daemon's tmux server is `-L cockpit` in tmux's per-user directory: a private one here.
export TMUX_TMPDIR="$scratch/tmux"
# Agents inherit the daemon's PATH: their `cockpit` must be this clone's, not the installed one.
export PATH="$bin:$PATH"
unset TMUX

require_binaries() {
    local name
    for name in cockpit cockpit-daemon cockpit-tui cockpit-claude-hook; do
        [[ -x $bin/$name ]] || {
            echo "$bin/$name is not built: cargo build --release --manifest-path $clone/Cargo.toml" >&2
            exit 1
        }
    done
}

enter_projects() {
    cd "$projects" 2>/dev/null || {
        echo "no scratch home at $scratch: run.sh start first" >&2
        exit 1
    }
}

daemon_pid() {
    local pid
    [[ -f $pid_file ]] || return 1
    pid=$(<"$pid_file")
    [[ $pid =~ ^[0-9]+$ ]] && [[ $(readlink "/proc/$pid/exe" 2>/dev/null) == "$bin/cockpit-daemon" ]] || return 1
    echo "$pid"
}

make_scratch() {
    mkdir -p "$scratch" "$XDG_CONFIG_HOME/cockpit" "$XDG_STATE_HOME" "$XDG_CACHE_HOME" \
        "$XDG_RUNTIME_DIR" "$TMUX_TMPDIR" "$projects"
    chmod 700 "$scratch" "$XDG_RUNTIME_DIR" "$TMUX_TMPDIR"
    touch "$scratch/$marker"
    # The shortest idle window the configuration accepts, so the walk can see a session sleep.
    [[ -f $XDG_CONFIG_HOME/cockpit/config.toml ]] ||
        printf '[sessions]\nidle_sleep_after_minutes = 30\n' >"$XDG_CONFIG_HOME/cockpit/config.toml"
    if [[ ! -d $demo/.git ]]; then
        mkdir -p "$demo"
        git -C "$demo" init --quiet --initial-branch=main
        printf '# demo\n\nA scratch project for the Cockpit acceptance walk.\n' >"$demo/README.md"
        git -C "$demo" add README.md
        git -C "$demo" -c user.name=acceptance -c user.email=acceptance@localhost \
            commit --quiet -m "Scratch project for the acceptance walk"
    fi
}

start_daemon() {
    local waited
    if daemon_pid >/dev/null; then
        echo "daemon already running (pid $(daemon_pid))"
        return
    fi
    # `setsid` in a non-interactive shell's background job execs in place, so $! is the daemon.
    setsid "$bin/cockpit-daemon" </dev/null >"$daemon_output" 2>&1 &
    echo $! >"$pid_file"
    disown
    for ((waited = 0; waited < 100; waited++)); do
        if grep -q '^ready ' "$daemon_output"; then
            grep '^ready ' "$daemon_output"
            return
        fi
        if ! daemon_pid >/dev/null; then
            echo "the daemon exited before it was ready:" >&2
            cat "$daemon_output" >&2
            exit 1
        fi
        sleep 0.1
    done
    echo "the daemon did not print ready within 10 s; its output is in $daemon_output" >&2
    exit 1
}

start() {
    require_binaries
    make_scratch
    start_daemon
    "$bin/cockpit" project list | grep -q "$demo" || "$bin/cockpit" project add "$demo" --name demo
    echo "scratch:  $scratch"
    echo "project:  $demo"
    echo "claude:   $(command -v claude || echo 'none on PATH: sessions cannot start')"
    echo "commit:   $(git -C "$clone" rev-parse HEAD)"
    echo "TUI:      bash $clone/tools/acceptance/run.sh tui"
}

# The newest session started in `demo` itself (not in a clone) that `verb` applies to.
newest_demo_session() {
    local verb=$1 list_flags=() states
    case $verb in
    sleep) states=running ;;
    wake) states=asleep ;;
    end) states='running|asleep' ;;
    resume) states=ended list_flags=(--ended) ;;
    esac
    "$bin/cockpit" session list "${list_flags[@]}" |
        awk -F '\t' -v dir="$demo" -v states="^($states)$" \
            '$6 == dir && $2 ~ states { id = $1 } END { if (id == "") exit 1; print id }'
}

stop() {
    local pid
    if pid=$(daemon_pid); then
        kill "$pid"
        while kill -0 "$pid" 2>/dev/null; do sleep 0.1; done
    fi
    rm -f "$pid_file"
    tmux -L cockpit kill-server 2>/dev/null || true
}

case ${1:-} in
start) start ;;
tui)
    require_binaries
    enter_projects
    exec "$bin/cockpit" tui
    ;;
cockpit)
    require_binaries
    shift
    enter_projects
    exec "$bin/cockpit" "$@"
    ;;
session)
    [[ ${2:-} =~ ^(sleep|wake|resume|end)$ ]] || {
        echo "$usage" >&2
        exit 2
    }
    require_binaries
    id=$(newest_demo_session "$2") || {
        echo "no session in $demo to $2" >&2
        exit 1
    }
    exec "$bin/cockpit" session "$2" "$id"
    ;;
kill-daemon)
    pid=$(daemon_pid) || {
        echo "no daemon of this clone is running" >&2
        exit 1
    }
    kill -9 "$pid"
    rm -f "$pid_file"
    echo "killed daemon $pid; its sessions keep running"
    ;;
stop) stop ;;
clean)
    [[ -f $scratch/$marker ]] || {
        echo "$scratch was not made by run.sh; not deleting it" >&2
        exit 1
    }
    stop
    rm -rf -- "$scratch"
    echo "removed $scratch"
    ;;
*)
    echo "$usage" >&2
    exit 2
    ;;
esac
