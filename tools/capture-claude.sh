#!/usr/bin/env bash
# Take the README screenshots of the Agent Cockpit into assets/, from a made-up cockpit:
# claude-demo build (apps/cockpit) writes invented sessions, todos, pull requests and usage into a
# temporary folder, a tmux server of its own (its own socket, -S) runs the real column on them beside a
# still picture of a session (claude-demo session), and a kitty window shows it on an
# empty workspace. Nothing of the real sessions, tasks or costs can appear; the real cockpit is
# not touched. Shots: the cockpit, the cockpit on Todo, and a guide window (ask-to-guide, made-up
# steps) over it. Runs outside the sandbox (grim, hyprctl, kitty), then returns.
set -uo pipefail

repo=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
out=$repo/assets
mkdir -p "$out"

demo=$(mktemp -d "${XDG_RUNTIME_DIR:-/tmp}/claude-demo.XXXXXX")
"$repo/apps/target/release/claude-demo" build "$demo" || exit 1
# tmux.conf sources its theme through ~: the fake home gets the real tmux folder.
mkdir -p "$demo/home/.config" && ln -s "$HOME/.config/tmux" "$demo/home/.config/tmux"
# The demo's own tmux server, by an explicit socket: inside tmux (the approval terminal) a client
# follows $TMUX and ignores TMUX_TMPDIR, so only -S keeps every command, kill-server included, off
# the real server.
demo_socket="$demo/tmux.sock"
demo_tmux() { tmux -S "$demo_socket" "$@"; }

# Started from a terminal that predates the running Hyprland (the approval terminal's tmux), the
# inherited instance id is stale: take the one whose socket is live, or stop.
for instance in $(ls -td "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"/hypr/*/ 2>/dev/null); do
    if [[ -S $instance/.socket.sock ]]; then
        export HYPRLAND_INSTANCE_SIGNATURE=$(basename "$instance")
        break
    fi
done
previous_workspace=$(hyprctl activeworkspace -j | jq -r .id) || { echo "capture-claude: hyprctl does not answer" >&2; exit 1; }
focus() { hyprctl dispatch "hl.dsp.focus({ workspace = $1 })" >/dev/null; }
# Only ever the demo: a shot is refused unless the demo's window (or the guide over it) has the focus,
# so a failed setup can never photograph the real screen. It covers the given windows only (the bar,
# with what is playing, and banners from real sessions stay out), at half size.
shot_windows() { # name class...
    local name=$1 focused geometry
    shift
    focused=$(hyprctl activewindow -j | jq -r '.class // empty')
    if [[ $focused != claude-demo && $focused != claude-guide ]]; then
        echo "capture-claude: the demo window is not on screen (focused: ${focused:-nothing}); no shot taken" >&2
        exit 1
    fi
    geometry=$(hyprctl clients -j | jq -r --args '[.[] | select(.class as $class | $ARGS.positional | index($class))]
        | if length == 0 then empty else
          (map(.at[0]) | min) as $x | (map(.at[1]) | min) as $y
          | "\($x),\($y) \((map(.at[0] + .size[0]) | max) - $x)x\((map(.at[1] + .size[1]) | max) - $y)" end' "$@")
    [[ -n $geometry ]] || { echo "capture-claude: no window to shoot for $name" >&2; exit 1; }
    grim -s 0.5 -g "$geometry" "$out/$name.png" && echo "assets/$name.png"
}
cleanup() {
    hyprctl dispatch 'hl.dsp.window.close({ window = "class:claude-guide" })' >/dev/null 2>&1
    hyprctl dispatch 'hl.dsp.window.close({ window = "class:claude-demo" })' >/dev/null 2>&1
    demo_tmux kill-server 2>/dev/null
    rm -rf "$demo"
    focus "$previous_workspace"
}
trap cleanup EXIT

# The demo cockpit, laid out like cockpit' ensure_manager: the column (60 columns) and the
# session pane with its title line.
env -u TMUX HOME="$demo/home" XDG_RUNTIME_DIR="$demo/run" XDG_STATE_HOME="$demo/state" PATH="$demo/bin:$PATH" \
    CLAUDE_STATUS_URL="file://$demo/status.json" \
    CODEX_STATUS_URL="file://$demo/status.json" \
    tmux -S "$demo_socket" -f "$HOME/.config/tmux/tmux.conf" new-session -d -s claude-manager -x 240 -y 60 "$HOME/.local/bin/cockpit-sidebar"
sidebar=$(demo_tmux list-panes -t claude-manager: -F '#{pane_id}' | head -n 1)
[[ $sidebar == %* ]] || { echo "capture-claude: the demo cockpit did not start" >&2; exit 1; }
demo_tmux set-option -p -t "$sidebar" @role sidebar
viewer=$(demo_tmux split-window -h -t "$sidebar" -l 70% -P -F '#{pane_id}' "$repo/apps/target/release/claude-demo" session)
demo_tmux set-option -p -t "$viewer" @role viewer
demo_tmux set-option -p -t "$viewer" @project acme-shop
demo_tmux select-pane -t "$viewer" -T "✳ Checkout totals with VAT per country"
demo_tmux resize-pane -t "$sidebar" -x 60
demo_tmux set-option -w -t claude-manager: pane-border-status top
demo_tmux set-option -w -t claude-manager: pane-border-format \
    '#{?#{==:#{@role},viewer}, #[fg=blue]#[bold]#{@project}#[nobold] · #{pane_title} #[default],}'
demo_tmux select-pane -t "$sidebar"

# An empty persistent workspace (1-5), as tools/capture.sh picks it.
scratch=$(hyprctl workspaces -j | jq -r '[.[] | select(.id > 0 and .id <= 5 and .windows > 0) | .id] as $busy | [range(1; 6)] - $busy | .[0]')
[[ $scratch =~ ^[1-5]$ ]] || { echo "capture-claude: no empty workspace" >&2; exit 1; }
focus "$scratch"
sleep 0.5
# Floating, below the banners' place at the top centre, leaving the right edge to the guide window.
hyprctl eval "hl.exec_cmd([[kitty --class claude-demo -e env -u TMUX tmux -S $demo_socket attach-session -t =claude-manager]], { float = true, size = \"2600 1300\", move = \"(monitor_w-3240) 200\" })" >/dev/null
for _ in $(seq 50); do [[ -n $(demo_tmux list-clients) ]] && break; sleep 0.1; done
sleep 1
# Once attached the window has the kitty's size: set the column's width and draw the session to fit.
demo_tmux resize-pane -t "$sidebar" -x 60
demo_tmux respawn-pane -k -t "$viewer" "$repo/apps/target/release/claude-demo" session
sleep 1.5

# 1. The cockpit
shot_windows claude-cockpit claude-demo

# 2. The cockpit on Todo
demo_tmux send-keys -t "$sidebar" 3
sleep 1.5
shot_windows claude-cockpit-todo claude-demo
demo_tmux send-keys -t "$sidebar" 1

# 3. A guide window over it: made-up steps, closed right after the shot (Claude-less: nobody waits).
ASK_TO_GUIDE_WAIT_S=20 ask-to-guide >/dev/null 2>&1 <<'EOF' &
# Search Console key
## Turn on the API
1. Open https://console.cloud.google.com/ and pick the project
2. Enable the Search Console API
   open: https://console.cloud.google.com/apis/library/searchconsole.googleapis.com
## Service account
3. Copy the new account's email
   copy: search-console-review@example.iam.gserviceaccount.com
4. Paste it here
   ask: Which email did it get?
EOF
sleep 4
shot_windows claude-guide claude-demo claude-guide
