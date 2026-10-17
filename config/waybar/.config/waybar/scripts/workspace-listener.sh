#!/bin/bash
# Keeps the bar hidden on the Claude workspace (the cockpit, hypr/apps/claude.lua) and shown
# everywhere else, following Hyprland's workspace events through waybar-visibility.sh, which
# owns the hide/show state. Same shape as audio-listener.sh.
socket="$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock"
visibility="$HOME/.config/waybar/scripts/waybar-visibility.sh"

# This script runs as a waybar module: waybar has just started and is visible, whatever the
# previous instance left behind, and may have started on the Claude workspace.
"$visibility" reset
"$visibility" sync
socat -U - "UNIX-CONNECT:$socket" | grep --line-buffered '^workspace>>' | while read -r _; do
    "$visibility" sync
done
