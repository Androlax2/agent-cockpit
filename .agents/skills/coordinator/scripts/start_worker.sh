#!/bin/sh
# Files a Torlek task item for an issue, then starts a worker whose prompt names that item.
# Usage: start_worker.sh ISSUE SLUG MODEL EFFORT "TASK TEXT"
# Environment: PROJECT_DIR (default: this git checkout), WORKER_AGENT (default: claude),
# WORKER_REPO (OWNER/NAME; default: the repository of the current directory).
set -eu

if [ "$#" -ne 5 ]; then
    echo 'usage: start_worker.sh ISSUE SLUG MODEL EFFORT "TASK TEXT"' >&2
    exit 2
fi
issue=$1
slug=$2
model=$3
effort=$4
task=$5

here=$(dirname "$0")
project_dir=${PROJECT_DIR:-$(git rev-parse --show-toplevel)}
repo=${WORKER_REPO:-$(gh repo view --json nameWithOwner --jq .nameWithOwner)}
item_title="#$issue $(gh issue view "$issue" -R "$repo" --json title --jq .title)"

torlek todo add "$item_title" --kind feature --notes "Worker branch greenfield/$issue-$slug" >/dev/null
# `todo add` prints no id, so take the newest item with exactly this title.
todo_id=$(torlek todo list | ITEM_TITLE=$item_title awk -F'\t' '$6 == ENVIRON["ITEM_TITLE"] {print $1}' | sort -n | tail -1)
[ -n "$todo_id" ] || {
    echo "no task item found for $item_title" >&2
    exit 1
}
suffix=$(sed "s/TODO_ID/$todo_id/g" "$here/../prompts/worker_suffix.txt")

echo "task item $todo_id"
torlek agent start "$project_dir" --agent "${WORKER_AGENT:-claude}" --clone "greenfield/$issue-$slug" \
    --model "$model" --effort "$effort" --prompt "$task $suffix"
