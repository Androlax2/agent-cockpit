#!/usr/bin/env bash
# Runs the new Cockpit for a live acceptance walk (docs/acceptance/m1.md #135, m2.md #136):
# the daemon, CLI and TUI built in this clone's target/release, on scratch XDG directories, a
# private tmux directory and the profile `acceptance`, so the installed Torlek, its state, its
# daemon and its sessions are never touched.
#
#   run.sh start              start the daemon, register the scratch project `demo`
#   run.sh tui                attach this terminal to the TUI
#   run.sh tui-window         open the TUI in its own opaque kitty window (Hyprland)
#   run.sh torlek ARGS...    run this clone's `torlek`, from the scratch projects directory
#   run.sh session new ARGS   start a session in `demo` (`torlek session new` options), remember it
#   run.sh session VERB ARGS  sleep|wake|resume|end|show|send|grant|grants|revoke the session
#                             `session new` remembered, with that verb's other arguments
#   run.sh fill-target BRANCH write 24 MiB of stand-in build output into the demo clone BRANCH's
#                             target/, for a storage report and a prune
#   run.sh prune-prepare BRANCH  prepare a prune plan for the demo clone BRANCH (it looks up the
#                             checkout number: removed clones keep theirs, so numbers vary)
#   run.sh big-needs on|off   on: each demo agent reserves 40% of the CPU budget (60% with the
#                             startup headroom), so one queued item fits at a time; off: none
#   run.sh todo-image ITEM FILE  attach FILE (relative to this clone) to item ITEM as an image note
#   run.sh code-change TITLE NOTES  file a demo item and start its code-change workflow
#   run.sh kill-daemon        kill -9 the daemon, leaving its sessions running
#   run.sh stop               stop the daemon and the scratch tmux server
#   run.sh clean              stop, then delete the scratch directory
#
# The scratch directory is $COCKPIT_ACCEPTANCE_HOME, by default /tmp/torlek-acceptance-<uid>:
# short, because the daemon's sockets live under it. It holds the configuration, the sockets,
# tmux and the scratch projects, all small. What grows (state, database, logs, run outputs,
# backups, cache) goes to $COCKPIT_ACCEPTANCE_STATE, by default this clone's
# target/acceptance: /tmp is a small shared tmpfs. HOME stays the owner's own, so the agent
# uses the owner's Claude Code login.
set -euo pipefail

usage="usage: run.sh start|tui|tui-window|kill-daemon|stop|clean | torlek ARGS... | session new [OPTIONS] | session sleep|wake|resume|end|show|send|grant|grants|revoke [ARGS...] | fill-target BRANCH | prune-prepare BRANCH | big-needs on|off | todo-image ITEM FILE | code-change TITLE NOTES"

clone=$(cd "$(dirname "$(readlink -f "$0")")/../.." && pwd)
bin="$clone/target/release"
scratch=${COCKPIT_ACCEPTANCE_HOME:-/tmp/torlek-acceptance-$(id -u)}
[[ $scratch == /* ]] || {
    echo "COCKPIT_ACCEPTANCE_HOME must be an absolute path: $scratch" >&2
    exit 2
}
scratch=$(realpath -m "$scratch")
bulk=${COCKPIT_ACCEPTANCE_STATE:-$clone/target/acceptance}
[[ $bulk == /* ]] || {
    echo "COCKPIT_ACCEPTANCE_STATE must be an absolute path: $bulk" >&2
    exit 2
}
bulk=$(realpath -m "$bulk")
projects="$scratch/projects"
demo="$projects/demo"
pid_file="$scratch/daemon.pid"
daemon_output="$bulk/daemon.out"
# `clean` deletes only a directory this script made.
marker=.torlek-acceptance

# `systemctl --user` finds the user manager through XDG_RUNTIME_DIR unless the bus address is
# set: keep the owner's, so sessions get the same systemd scope containment as outside the walk.
owner_runtime=${XDG_RUNTIME_DIR:-}
if [[ -z ${DBUS_SESSION_BUS_ADDRESS:-} && -n $owner_runtime && -S $owner_runtime/bus ]]; then
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$owner_runtime/bus"
fi
export XDG_CONFIG_HOME="$scratch/config"
export XDG_STATE_HOME="$bulk/state"
export XDG_CACHE_HOME="$bulk/cache"
export XDG_RUNTIME_DIR="$scratch/runtime"
# A profile of its own on top of the scratch directories: every file, socket and tmux server
# is named torlek-acceptance, never torlek, whatever TORLEK_PROFILE the caller had.
export TORLEK_PROFILE=acceptance
# The daemon's tmux server is $TMUX_TMPDIR/tmux-<uid>/torlek-<profile>, which the daemon names
# by path and refuses to start without (#845): a private one here.
export TMUX_TMPDIR="$scratch/tmux"
# Agents inherit the daemon's PATH: their `torlek` must be this clone's, not the installed one,
# and their `claude` the one start linked into $scratch/bin (see link_claude).
owner_path=$PATH
export PATH="$scratch/bin:$bin:$PATH"
unset TMUX
# A session of the installed Torlek (the facilitator's, or the owner's terminal inside one)
# must not lend the walk its identity or its GitHub token: the walk acts as the owner of the
# scratch daemon, and its agents get no token unless a step sets one.
unset TORLEK_SESSION TORLEK_LAUNCH_TOKEN TORLEK_INSTRUCTIONS GH_TOKEN GITHUB_TOKEN

require_binaries() {
    local name
    for name in torlek torlek-daemon torlek-tui torlek-claude-hook; do
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
    [[ ${program% (deleted)} == "$bin/torlek-daemon" ]] || return 1
    echo "$pid"
}

# Refuses a directory that exists with something in it but no marker: `clean` would later
# delete what the marker claimed.
claim() {
    local dir=$1 entries
    [[ -e $dir && ! -f $dir/$marker ]] || return 0
    entries=$(
        shopt -s nullglob dotglob
        echo "$dir"/*
    )
    [[ -z $entries ]] || {
        echo "$dir exists and was not made by run.sh; choose another directory" >&2
        exit 1
    }
}

make_scratch() {
    claim "$scratch"
    claim "$bulk"
    mkdir -p "$scratch" "$bulk" "$XDG_CONFIG_HOME/torlek-$TORLEK_PROFILE" "$XDG_STATE_HOME" \
        "$XDG_CACHE_HOME" "$XDG_RUNTIME_DIR" "$TMUX_TMPDIR" "$projects"
    chmod 700 "$scratch" "$bulk" "$XDG_RUNTIME_DIR" "$TMUX_TMPDIR"
    touch "$scratch/$marker" "$bulk/$marker"
    # The shortest idle window the configuration accepts, so the walk can see a session sleep.
    # Agents run unsandboxed, as in the M1 walk: a sandbox would need this profile's own engines
    # and a managed proxy, whose fixed ports the installed Torlek's proxy may hold. A clone whose
    # branch main contains may have its target/ pruned, so the walk can prune one.
    [[ -f $XDG_CONFIG_HOME/torlek-$TORLEK_PROFILE/config.toml ]] ||
        printf '%s\n' '[sessions]' 'idle_sleep_after_minutes = 30' '' \
            '[agents]' 'sandbox = "off"' '' \
            '[storage]' 'target_prune_base_ref = "refs/heads/main"' \
            >"$XDG_CONFIG_HOME/torlek-$TORLEK_PROFILE/config.toml"
    if [[ ! -d $demo/.git ]]; then
        mkdir -p "$demo"
        git -C "$demo" init --quiet --initial-branch=main
        printf '# demo\n\nA scratch project for the Cockpit acceptance walk.\n' >"$demo/README.md"
        # Ignored build output, as in a real project: prune refuses a target/ git would track.
        printf 'target/\n' >"$demo/.gitignore"
        git -C "$demo" add README.md .gitignore
        git -C "$demo" -c user.name=acceptance -c user.email=acceptance@localhost \
            commit --quiet -m "Scratch project for the acceptance walk"
        # The walk's own project settings stay out of the agents' git status.
        printf '.torlek/\n' >>"$demo/.git/info/exclude"
        write_demo_config 0
    fi
}

# The demo project's .torlek/config.toml: the check the code-change preset's `checks`
# requirement names (`review` is left undeclared, so only the owner can waive it), and, when
# cpu_millis is not 0, what each agent's work reserves.
write_demo_config() {
    local cpu_millis=$1
    mkdir -p "$demo/.torlek"
    {
        printf '[[checks]]\nname = "checks"\ncommand = "./greet.sh | grep -qi hello"\n'
        if ((cpu_millis > 0)); then
            printf '\n[needs]\ncpu_millis = %d\n' "$cpu_millis"
        fi
    } >"$demo/.torlek/config.toml"
}

# 40% of the budget `torlek resources status` reports: with the 50% startup headroom one item
# takes 60%, so a second waits until the first is done.
big_needs() {
    local budget
    budget=$("$bin/torlek" resources status | awk '$1 == "budget" { print $2; exit }')
    [[ $budget =~ ^[0-9]+$ ]] || {
        echo "torlek resources status printed no budget" >&2
        exit 1
    }
    write_demo_config $((budget * 2 / 5))
    echo "each demo agent now reserves $((budget * 2 / 5)) of $budget millicores"
}

# A clone's directory is its branch with / as - (cockpit-projects' clones.rs); anything else,
# such as .. or a leading dot, is refused so the path stays under .parallel/demo.
clone_directory() {
    local name=${1//\//-}
    [[ $name =~ ^[A-Za-z0-9_][A-Za-z0-9._-]*$ ]] || {
        echo "not a branch name run.sh accepts: $1" >&2
        exit 2
    }
    echo "$projects/.parallel/demo/$name"
}

fill_target() {
    local branch=$1 checkout index
    checkout=$(clone_directory "$branch")
    [[ -d $checkout/.git || -f $checkout/.git ]] || {
        echo "no demo clone $branch at $checkout: run.sh torlek checkout clone demo $branch first" >&2
        exit 1
    }
    mkdir -p "$checkout/target/debug"
    for index in 1 2 3; do
        head -c 8M /dev/urandom >"$checkout/target/debug/stand-in-$index"
    done
    echo "wrote 25165824 bytes into $checkout/target"
}

prune_prepare() {
    local branch=$1 checkout
    clone_directory "$branch" >/dev/null
    checkout=$("$bin/torlek" checkout list demo |
        awk -F '\t' -v kind="parallel:$branch" '$3 == kind { print $1 }')
    [[ $checkout =~ ^[0-9]+$ ]] || {
        echo "no single demo clone $branch in torlek checkout list demo" >&2
        exit 1
    }
    echo "demo clone $branch is checkout $checkout"
    exec "$bin/torlek" storage prune "$checkout"
}

start_daemon() {
    local pid waited
    if pid=$(daemon_pid); then
        echo "daemon already running (pid $pid)"
        return
    fi
    # `setsid` in a non-interactive shell's background job execs in place, so $! is the daemon.
    setsid "$bin/torlek-daemon" </dev/null >"$daemon_output" 2>&1 &
    pid=$!
    echo "$pid" >"$pid_file"
    disown
    for ((waited = 0; waited < 300; waited++)); do
        grep '^ready ' "$daemon_output" && return
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "the daemon exited before it was ready:" >&2
            cat "$daemon_output" >&2
            exit 1
        fi
        sleep 0.1
    done
    echo "the daemon did not print ready within 30 s; its output is in $daemon_output" >&2
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
    if ! "$bin/torlek" project list | awk -F '\t' -v dir="$demo" '$2 == dir { found = 1 } END { exit !found }'; then
        "$bin/torlek" project add "$demo" --name demo
    fi
    echo "scratch:  $scratch"
    echo "state:    $bulk"
    echo "profile:  $TORLEK_PROFILE"
    echo "project:  $demo"
    echo "claude:   $(readlink "$scratch/bin/claude") (outside greywall)"
    echo "commit:   $(git -C "$clone" rev-parse HEAD)"
    echo "TUI:      bash $clone/tools/acceptance/run.sh tui"
}

# Kills the scratch tmux server NAME by its socket path. Never `tmux -L`: when TMUX_TMPDIR's
# directory is missing, tmux falls back to /tmp, where the installed Torlek's server lives.
stop_tmux() {
    local socket
    socket="$TMUX_TMPDIR/tmux-$(id -u)/$1"
    [[ -S $socket ]] || return 0
    tmux -S "$socket" kill-server 2>/dev/null || true
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
    stop_tmux "torlek-$TORLEK_PROFILE"
    # An M1-era scratch home's daemon had no profile: its server is named torlek.
    stop_tmux torlek
}

# The owner's Hyprland draws windows at 0.95 / 0.9 opacity, too faint for the walk: the TUI gets
# its own kitty window, made opaque by a rule on its class (the same rule as hypr/apps/craft.lua),
# added for this Hyprland session only.
tui_window() {
    local class=torlek-acceptance rule
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
    exec "$bin/torlek" tui
    ;;
torlek)
    require_binaries
    shift
    enter_projects
    exec "$bin/torlek" "$@"
    ;;
session)
    case ${2:-} in
    new)
        require_binaries
        enter_projects
        line=$("$bin/torlek" session new demo "${@:3}")
        echo "$line"
        echo "${line%%$'\t'*}" >"$scratch/session"
        ;;
    sleep | wake | resume | end | show | send | grant | grants | revoke)
        require_binaries
        enter_projects
        [[ -s $scratch/session ]] || {
            echo "no session started with run.sh session new yet" >&2
            exit 1
        }
        exec "$bin/torlek" session "$2" "$(<"$scratch/session")" "${@:3}"
        ;;
    *)
        echo "$usage" >&2
        exit 2
        ;;
    esac
    ;;
fill-target)
    [[ $# -eq 2 ]] || {
        echo "$usage" >&2
        exit 2
    }
    enter_projects
    fill_target "$2"
    ;;
prune-prepare)
    [[ $# -eq 2 ]] || {
        echo "$usage" >&2
        exit 2
    }
    require_binaries
    enter_projects
    prune_prepare "$2"
    ;;
big-needs)
    require_binaries
    enter_projects
    case ${2:-} in
    on) big_needs ;;
    off)
        write_demo_config 0
        echo "demo agents reserve nothing of their own"
        ;;
    *)
        echo "$usage" >&2
        exit 2
        ;;
    esac
    ;;
todo-image)
    [[ $# -eq 3 ]] || {
        echo "$usage" >&2
        exit 2
    }
    require_binaries
    image=$3
    [[ $image == /* ]] || image="$clone/$image"
    enter_projects
    exec "$bin/torlek" todo image add "$2" "$image"
    ;;
code-change)
    [[ $# -eq 3 ]] || {
        echo "$usage" >&2
        exit 2
    }
    require_binaries
    enter_projects
    filed=$("$bin/torlek" todo add "$2" --project demo --kind feature --notes "$3")
    echo "$filed"
    item=$(awk 'NR == 1 && $1 == "item" { print $2 }' <<<"$filed")
    [[ $item =~ ^[0-9]+$ ]] || {
        echo "torlek todo add printed no item number" >&2
        exit 1
    }
    exec "$bin/torlek" workflow start "$item" --preset code-change
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
    for made in "$scratch" "$bulk"; do
        [[ ! -e $made || -f $made/$marker ]] || {
            echo "$made was not made by run.sh; not deleting it" >&2
            exit 1
        }
    done
    stop
    rm -rf -- "$scratch" "$bulk"
    echo "removed $scratch and $bulk"
    ;;
*)
    echo "$usage" >&2
    exit 2
    ;;
esac
