#!/usr/bin/env bash
# Runs the new Cockpit for a live acceptance walk (docs/acceptance/m1.md, #135): the daemon,
# CLI and TUI built in this clone's target/release, on scratch XDG directories and a private
# tmux directory, so the installed (legacy) cockpit and its state are never touched.
#
#   run.sh start              start the daemon, register the scratch project `demo`
#   run.sh tui                attach this terminal to the TUI
#   run.sh tui-window         open the TUI in its own opaque kitty window (Hyprland)
#   run.sh cockpit ARGS...    run this clone's `cockpit`, from the scratch projects directory
#   run.sh session new ARGS   start a session in `demo` (`cockpit session new` options), remember it
#   run.sh session VERB       sleep|wake|resume|end|show the session `session new` remembered
#   run.sh kill-daemon        kill -9 the daemon, leaving its sessions running
#   run.sh stop               stop the daemon and the scratch tmux server
#   run.sh clean              stop, then delete the scratch directory
#
# The scratch directory is $COCKPIT_ACCEPTANCE_HOME, by default /tmp/cockpit-acceptance-<uid>:
# short, because the daemon's sockets live under it. HOME stays the owner's own, so the agent
# uses the owner's Claude Code login.
set -euo pipefail

usage="usage: run.sh start|tui|tui-window|kill-daemon|stop|clean | cockpit ARGS... | session new [OPTIONS] | session sleep|wake|resume|end|show"

clone=$(cd "$(dirname "$(readlink -f "$0")")/../.." && pwd)
bin="$clone/target/release"
scratch=${COCKPIT_ACCEPTANCE_HOME:-/tmp/cockpit-acceptance-$(id -u)}
[[ $scratch == /* ]] || {
    echo "COCKPIT_ACCEPTANCE_HOME must be an absolute path: $scratch" >&2
    exit 2
}
scratch=$(realpath -m "$scratch")
projects="$scratch/projects"
demo="$projects/demo"
pid_file="$scratch/daemon.pid"
daemon_output="$scratch/daemon.out"
# `clean` deletes only a directory this script made.
marker=.cockpit-acceptance

# `systemctl --user` finds the user manager through XDG_RUNTIME_DIR unless the bus address is
# set: keep the owner's, so sessions get the same systemd scope containment as outside the walk.
owner_runtime=${XDG_RUNTIME_DIR:-}
if [[ -z ${DBUS_SESSION_BUS_ADDRESS:-} && -n $owner_runtime && -S $owner_runtime/bus ]]; then
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$owner_runtime/bus"
fi
export XDG_CONFIG_HOME="$scratch/config"
export XDG_STATE_HOME="$scratch/state"
export XDG_CACHE_HOME="$scratch/cache"
export XDG_RUNTIME_DIR="$scratch/runtime"
# The daemon's tmux server is `-L cockpit` in tmux's per-user directory: a private one here.
export TMUX_TMPDIR="$scratch/tmux"
# Agents inherit the daemon's PATH: their `cockpit` must be this clone's, not the installed one,
# and their `claude` the one start linked into $scratch/bin (see link_claude).
owner_path=$PATH
export PATH="$scratch/bin:$bin:$PATH"
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

# The recorded daemon, when that process still runs this clone's daemon (a rebuild leaves the
# running one as "<path> (deleted)").
daemon_pid() {
    local pid program
    [[ -f $pid_file ]] || return 1
    pid=$(<"$pid_file")
    [[ $pid =~ ^[0-9]+$ ]] || return 1
    program=$(readlink "/proc/$pid/exe" 2>/dev/null) || return 1
    [[ ${program% (deleted)} == "$bin/cockpit-daemon" ]] || return 1
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
    local pid waited
    if pid=$(daemon_pid); then
        echo "daemon already running (pid $pid)"
        return
    fi
    # `setsid` in a non-interactive shell's background job execs in place, so $! is the daemon.
    setsid "$bin/cockpit-daemon" </dev/null >"$daemon_output" 2>&1 &
    pid=$!
    echo "$pid" >"$pid_file"
    disown
    for ((waited = 0; waited < 100; waited++)); do
        grep '^ready ' "$daemon_output" && return
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "the daemon exited before it was ready:" >&2
            cat "$daemon_output" >&2
            exit 1
        fi
        sleep 0.1
    done
    echo "the daemon did not print ready within 10 s; its output is in $daemon_output" >&2
    exit 1
}

# The daemon launches the `claude` on its PATH. A greywall launcher there would hide this clone's
# target/release and the scratch sockets from the agent, so the walk runs Claude Code itself,
# unsandboxed in the scratch project: COCKPIT_ACCEPTANCE_CLAUDE, else the first `claude` on the
# owner's PATH outside a greywall-agents directory.
link_claude() {
    local claude=${COCKPIT_ACCEPTANCE_CLAUDE:-} candidate
    if [[ -n $claude ]]; then
        claude=$(readlink -f "$claude")
    else
        while read -r candidate; do
            # Resolved, so a link to the launcher is skipped too.
            candidate=$(readlink -f "$candidate")
            [[ $candidate == */greywall-agents/* ]] || {
                claude=$candidate
                break
            }
        done < <(PATH=$owner_path type -ap claude)
    fi
    [[ -n $claude && -x $claude ]] || {
        echo "no claude outside greywall on PATH; set COCKPIT_ACCEPTANCE_CLAUDE" >&2
        exit 1
    }
    mkdir -p "$scratch/bin"
    ln -sfn "$claude" "$scratch/bin/claude"
}

start() {
    require_binaries
    make_scratch
    link_claude
    start_daemon
    if ! "$bin/cockpit" project list | awk -F '\t' -v dir="$demo" '$2 == dir { found = 1 } END { exit !found }'; then
        "$bin/cockpit" project add "$demo" --name demo
    fi
    echo "scratch:  $scratch"
    echo "project:  $demo"
    echo "claude:   $(readlink "$scratch/bin/claude") (outside greywall)"
    echo "commit:   $(git -C "$clone" rev-parse HEAD)"
    echo "TUI:      bash $clone/tools/acceptance/run.sh tui"
}

stop() {
    local pid waited
    if pid=$(daemon_pid); then
        kill "$pid"
        for ((waited = 0; waited < 100; waited++)); do
            kill -0 "$pid" 2>/dev/null || break
            sleep 0.1
        done
        if kill -0 "$pid" 2>/dev/null; then
            echo "the daemon did not stop within 10 s; killing it" >&2
            kill -9 "$pid"
        fi
    fi
    rm -f "$pid_file"
    tmux -L cockpit kill-server 2>/dev/null || true
}

# The owner's Hyprland draws windows at 0.95 / 0.9 opacity, too faint for the walk: the TUI gets
# its own kitty window, made opaque by a rule on its class (the same rule as hypr/apps/craft.lua),
# added for this Hyprland session only.
tui_window() {
    local class=cockpit-acceptance rule
    require_binaries
    enter_projects
    rule="hl.window_rule({ match = { class = \"^($class)\$\" }, opacity = \"1 1\" })"
    # hyprctl and kitty find the compositor through the owner's runtime directory.
    XDG_RUNTIME_DIR=$owner_runtime hyprctl eval "$rule" >/dev/null || {
        echo "hyprctl could not make the window opaque; nothing opened" >&2
        exit 1
    }
    # kitty reads the owner's own configuration; `run.sh tui` sets the scratch home again inside it.
    env -u XDG_CONFIG_HOME -u XDG_STATE_HOME -u XDG_CACHE_HOME -u TMUX_TMPDIR \
        XDG_RUNTIME_DIR="$owner_runtime" PATH="$owner_path" \
        setsid -f kitty --class "$class" bash "$clone/tools/acceptance/run.sh" tui \
        </dev/null >/dev/null 2>&1
    echo "opened the TUI in an opaque kitty window (class $class)"
}

case ${1:-} in
start) start ;;
tui-window) tui_window ;;
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
    case ${2:-} in
    new)
        require_binaries
        enter_projects
        line=$("$bin/cockpit" session new demo "${@:3}")
        echo "$line"
        echo "${line%%$'\t'*}" >"$scratch/session"
        ;;
    sleep | wake | resume | end | show)
        require_binaries
        enter_projects
        [[ -s $scratch/session ]] || {
            echo "no session started with run.sh session new yet" >&2
            exit 1
        }
        exec "$bin/cockpit" session "$2" "$(<"$scratch/session")"
        ;;
    *)
        echo "$usage" >&2
        exit 2
        ;;
    esac
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
