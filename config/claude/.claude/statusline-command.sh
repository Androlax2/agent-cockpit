#!/usr/bin/env bash
# Claude Code status line: "[user@host project]" in the prompt bar, and a copy of the per-session
# facts the manager sidebar shows (model, context used, cost, /rename name) dropped in
# ~/.cache/claude, the one cache dir greywall lets a session write.
input=$(cat)
currentDir=$(jq -r '.workspace.current_dir // .cwd' <<<"$input")
# bash's HOSTNAME: the sandbox has no hostname binary, which left the bar reading "theo@ project".
printf '[%s@%s %s]' "$(whoami)" "${HOSTNAME%%.*}" "$(basename "$currentDir")"

session=$(jq -r '.session_id // empty' <<<"$input")
[[ -n $session ]] || exit 0
cache_dir="$HOME/.cache/claude"
mkdir -p "$cache_dir"
# CLAUDE_EFFORT is the session's effort level, which Claude exports to the commands it runs.
jq -c --arg effort "${CLAUDE_EFFORT:-}" '{model: .model.display_name, context: .context_window.used_percentage, cost: .cost.total_cost_usd,
        name: .session_name, remote: (.remote != null), effort: (if $effort == "" then null else $effort end), updated: now | floor}' <<<"$input" > "$cache_dir/statusline-$session.json.tmp" \
    && mv "$cache_dir/statusline-$session.json.tmp" "$cache_dir/statusline-$session.json"
