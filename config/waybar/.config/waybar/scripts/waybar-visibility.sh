#!/bin/bash
# The bar's visibility: hidden on the Claude workspace (the cockpit, hypr/apps/claude.lua), shown
# elsewhere. waybar's SIGUSR1 is a stateless toggle: the current state is read from Hyprland's
# layers, with a runtime file as the fallback, and every sender goes through here: workspace-listener.sh on each workspace event, and
# claude-sessions before it switches to the cockpit (hiding first avoids a flash of the bar).
#   waybar-visibility.sh sync    hide or show according to the active workspace
#   waybar-visibility.sh hide
#   waybar-visibility.sh show
#   waybar-visibility.sh reset   waybar just started, so it is visible
STATE="$XDG_RUNTIME_DIR/waybar-visible"
on_claude_workspace="$HOME/.config/waybar/scripts/on-claude-workspace.sh"

# What Hyprland shows: waybar's SIGUSR1 moves the bar from the top layer (shown) to the bottom one
# (behind the windows) and back. 1 shown, 0 hidden, nothing when Hyprland does not say.
on_screen() {
    local level
    level=$(hyprctl layers 2>/dev/null | awk '/Layer level/ { level = $3 } /namespace: waybar/ { print level; exit }')
    case $level in
        2 | 3) echo 1 ;;
        0 | 1) echo 0 ;;
    esac
}

# The stored state alone went wrong once a toggle got lost (one sent at boot, before waybar
# listened): every later switch then flipped the bar the wrong way. Hyprland's answer comes first.
current() {
    local shown
    shown=$(on_screen)
    if [[ -n $shown ]]; then echo "$shown"; else cat "$STATE" 2>/dev/null || echo 1; fi
}

set_visible() {
    local tries
    if [[ $(current) != "$1" ]]; then
        # -x: the exact process name. Without it the pattern also matches this script's own name.
        pkill -x -USR1 waybar || true
        printf '%s\n' "$1" > "$STATE"
        # Until the move shows, a quick next event would read the old layer and flip it back.
        for (( tries = 0; tries < 10; tries++ )); do
            [[ -z $(on_screen) || $(on_screen) == "$1" ]] && break
            sleep 0.05
        done
    fi
}

case ${1:-sync} in
    sync) if "$on_claude_workspace"; then set_visible 0; else set_visible 1; fi ;;
    hide) set_visible 0 ;;
    show) set_visible 1 ;;
    reset) printf '1\n' > "$STATE" ;;
    *) echo "usage: waybar-visibility.sh sync|hide|show|reset" >&2; exit 2 ;;
esac
