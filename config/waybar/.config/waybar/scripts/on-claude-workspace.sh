#!/usr/bin/env bash
# Exit 0 while the Claude workspace (6, see hypr/apps/claude.lua) is the active one. The bar hides
# itself there (scripts/workspace-listener.sh) and claude-sessions show toggles back from it.
CLAUDE_WORKSPACE=6  # the same number as WORKSPACE in hypr/apps/claude.lua and CLAUDE_WORKSPACE in hypr/scratchpads.lua
[[ $(hyprctl -j activeworkspace 2>/dev/null | jq -r '.id' 2>/dev/null) == "$CLAUDE_WORKSPACE" ]]
