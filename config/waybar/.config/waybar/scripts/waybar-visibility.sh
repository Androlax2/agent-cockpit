#!/bin/bash
# The bar's visibility: hidden on the Claude workspace (the cockpit, hypr/apps/claude.lua), shown
# elsewhere. waybar's SIGUSR1 is a stateless toggle, so the current state is kept in a runtime
# file that every sender goes through: workspace-listener.sh on each workspace event, and
# claude-sessions before it switches to the cockpit (hiding first avoids a flash of the bar).
#   waybar-visibility.sh sync    hide or show according to the active workspace
#   waybar-visibility.sh hide
#   waybar-visibility.sh show
#   waybar-visibility.sh reset   waybar just started, so it is visible
STATE="$XDG_RUNTIME_DIR/waybar-visible"
on_claude_workspace="$HOME/.config/waybar/scripts/on-claude-workspace.sh"

current() { cat "$STATE" 2>/dev/null || echo 1; }

set_visible() {
    if [[ $(current) != "$1" ]]; then
        # -x: the exact process name. Without it the pattern also matches this script's own name.
        pkill -x -USR1 waybar || true
        printf '%s\n' "$1" > "$STATE"
    fi
}

case ${1:-sync} in
    sync) if "$on_claude_workspace"; then set_visible 0; else set_visible 1; fi ;;
    hide) set_visible 0 ;;
    show) set_visible 1 ;;
    reset) printf '1\n' > "$STATE" ;;
    *) echo "usage: waybar-visibility.sh sync|hide|show|reset" >&2; exit 2 ;;
esac
