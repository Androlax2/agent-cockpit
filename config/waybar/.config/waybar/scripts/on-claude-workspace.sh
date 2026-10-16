#!/usr/bin/env bash
# Exit 0 while the Claude workspace (6, see hypr/apps/claude.lua) is the active one. The Claude
# modules in the bar's centre island are only shown there; hypr/apps/claude.lua signals the bar
# on every workspace change so they appear and vanish at once.
CLAUDE_WORKSPACE=6
[[ $(hyprctl -j activeworkspace 2>/dev/null | jq -r '.id') == "$CLAUDE_WORKSPACE" ]]
