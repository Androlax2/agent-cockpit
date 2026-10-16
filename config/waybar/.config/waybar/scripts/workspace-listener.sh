#!/bin/bash
# Forward Hyprland workspace switches to the bar as SIGRTMIN+15, so the Claude usage modules
# (shown only on the Claude workspace, see on-claude-workspace.sh) appear and vanish at once
# instead of on their next interval. Same shape as audio-listener.sh.
socket="$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock"
socat -U - "UNIX-CONNECT:$socket" | grep --line-buffered '^workspace>>' | while read -r _; do
    pkill -RTMIN+15 waybar
done
