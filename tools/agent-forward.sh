#!/usr/bin/env bash
# Forwards host ports (a dev server, a Sail app) into a project's sandboxes for DAYS days, by
# writing the host-only file the launcher reads at each session start (launcher-common.sh):
#   ~/.config/greywall-agents/forward/<project>  ->  "<expiry epoch> <port>..."
# Running sessions keep the forwards they started with: restart them to get the new ones.
#   tools/agent-forward.sh <project> <days> <port>...
set -euo pipefail

usage="usage: agent-forward.sh <project folder in ~/IdeaProjects> <days> <port>..."
project=${1:?$usage}
days=${2:?$usage}
shift 2
(( $# > 0 )) || { echo "$usage" >&2; exit 2; }

[[ $project =~ ^[A-Za-z0-9._-]+$ && $project != .* ]] || { echo "not a project folder name: $project" >&2; exit 2; }
[[ $days =~ ^[0-9]+$ ]] && (( days >= 1 && days <= 90 )) || { echo "days must be between 1 and 90: $days" >&2; exit 2; }
for port in "$@"; do
    [[ $port =~ ^[0-9]+$ ]] && (( port >= 1024 && port <= 65535 )) || { echo "not a port between 1024 and 65535: $port" >&2; exit 2; }
done

forward_dir=${AGENT_FORWARD_DIR:-$HOME/.config/greywall-agents/forward}
forward_file="$forward_dir/$project"
mkdir -p "$forward_dir"
if [[ -r $forward_file ]]; then
    echo "replacing: $(cat "$forward_file")"
fi
expiry=$(( $(date +%s) + days * 86400 ))
printf '%s %s\n' "$expiry" "$*" > "$forward_file.new"
mv "$forward_file.new" "$forward_file"
echo "$project sandboxes get localhost:${*// /, localhost:} until $(date -d "@$expiry" '+%Y-%m-%d %H:%M'); restart its sessions to apply."
