#!/usr/bin/env bash
# SessionStart hook: tells each new Claude session what this sandbox gives it for the current
# project (Docker daemon, GitHub token, image paste) and which `make` command Theo can run when
# something is missing. Its stdout is added to the session's context. It only inspects
# variables and sockets the launcher (~/.local/bin/greywall-agents/claude) prepares, so it is
# instant and never fails the session start.
set -uo pipefail

DOTFILES=~/dotfiles
PROJECTS_DIR=/home/theo/IdeaProjects

payload=$(cat)
cwd=$(jq -r '.cwd // empty' <<<"$payload" 2>/dev/null)
cwd=${cwd:-$PWD}
project_root=$(git -C "$cwd" rev-parse --show-toplevel 2>/dev/null || echo "$cwd")
# A parallel clone (claude-sessions' +, <clones>/<repo>/<name>) is its repository's project, as in
# the launcher: same GitHub token, same name.
clones_dir="${CLAUDE_SESSIONS_CLONES:-$HOME/IdeaProjects/.parallel}"
if [[ $project_root == "$clones_dir"/*/* ]]; then
    project_name=$(basename "$(dirname "$project_root")")
else
    project_name=$(basename "$project_root")
fi

describe_sandbox() {
    if [[ ${GREYWALL_SANDBOX:-} == 1 ]]; then
        echo "- Sandbox: greywall is active. Most of the home folder is hidden; never probe for escapes."
    else
        echo "- Sandbox: NOT ACTIVE. This session was started outside greywall. Tell Theo before doing anything else, and do not run commands until Theo confirms."
    fi
}

describe_docker() {
    if [[ -n ${DOCKER_HOST:-} && -S ${DOCKER_HOST#unix://} ]]; then
        echo "- Docker: ready, this project's own rootless daemon (${DOCKER_HOST})."
    elif [[ $project_root == "$PROJECTS_DIR"/* ]]; then
        echo "- Docker: not set up for ${project_name}. If the task needs Docker, ask Theo to run \`make -C ${DOTFILES} agent-docker-add PROJECT=${project_name}\`, then restart the session."
    else
        echo "- Docker: unavailable. Isolated daemons only exist for projects directly under ${PROJECTS_DIR}."
    fi
}

describe_github() {
    local remote repository
    remote=$(git -C "$project_root" remote get-url origin 2>/dev/null || true)
    repository=${remote%.git}
    repository=${repository#https://github.com/}
    repository=${repository#git@github.com:}
    if [[ -z $repository ]]; then
        repository="the project repository"
    fi
    if [[ -n ${GH_TOKEN:-} ]]; then
        echo "- GitHub: ready, a token limited to ${repository}. gh and git push work; push feature branches only."
    elif [[ $remote == *github.com* ]]; then
        echo "- GitHub: no token for ${project_name}. If the task needs GitHub, ask Theo to run \`make -C ${DOTFILES} agent-github-token PROJECT=${project_name}\`, then restart the session."
    else
        echo "- GitHub: no GitHub remote for this project."
    fi
}

describe_image_paste() {
    if [[ -S ${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/clipboard-image/clipboard.sock ]]; then
        echo "- Image paste: available through the PNG-only clipboard bridge."
    else
        echo "- Image paste: unavailable. Ask Theo to run \`systemctl --user enable --now clipboard-image-bridge\`, then restart the session."
    fi
}

# Designs in FigCraft: the interface is designed before it is built (the design-first skill).
describe_design() {
    local designs
    designs=$(git -C "$project_root" ls-files --cached --others --exclude-standard -- '*.figcraft' 2>/dev/null | head -3 | paste -sd ',' | sed 's/,/, /g')
    [[ -n $designs ]] || return 0
    echo "- Design: FigCraft files (${designs}). Before implementing or changing any interface, load the design-first skill; design work in FigCraft itself starts with figcraft-design."
}

# Whether ask-to-run (and ask-to-guide, claude-todo) can reach Theo from this sandbox: a session
# that cannot finds out now, with the reason and its fix, not when a command is waiting.
describe_ask_to_run() {
    local check
    command -v ask-to-run >/dev/null || { echo "- ask-to-run: not on PATH. Tell Theo; give him commands as fish blocks meanwhile."; return; }
    if check=$(timeout 5 ask-to-run --check 2>&1); then
        echo "- ask-to-run: ready."
    else
        echo "- ask-to-run: NOT WORKING. ${check:-no answer within 5 s.} Tell Theo now, with that fix."
    fi
}

echo "Sandbox status for ${project_name} (from ~/.claude/hooks/agent-environment.sh):"
describe_sandbox
describe_docker
describe_github
describe_image_paste
describe_design
describe_ask_to_run
exit 0
