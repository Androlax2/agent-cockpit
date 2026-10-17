#!/usr/bin/env bash
# Exit 0 while the Claude workspace (6, see hypr/apps/claude.lua) is the active one. The bar hides
# itself there (scripts/workspace-listener.sh) and claude-sessions show toggles back from it.
CLAUDE_WORKSPACE=6
[[ $(hyprctl -j activeworkspace 2>/dev/null | jq -r '.id') == "$CLAUDE_WORKSPACE" ]]
