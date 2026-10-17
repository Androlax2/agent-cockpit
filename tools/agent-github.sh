#!/usr/bin/env bash
# GitHub access for coding agents: one fine-grained token per project in ~/IdeaProjects, kept in
# the keyring (service agent-gh, project <name>). The greywall launcher exports it as GH_TOKEN for
# sessions started in that project; the keyring itself stays hidden from the sandbox.
# A second, classic token with only the project scope (service agent-gh-projects) is shared by
# every project: fine-grained tokens cannot reach Project boards owned by a user account.
# GitHub has no API to create tokens or revoke the CLI's login, so those steps open a pre-filled
# page and verify the result.
set -euo pipefail

PROJECTS_DIR=/home/theo/IdeaProjects
NEW_TOKEN_URL=https://github.com/settings/personal-access-tokens/new
NEW_CLASSIC_TOKEN_URL=https://github.com/settings/tokens/new
AUTHORIZED_APPS_URL=https://github.com/settings/applications
RULESET_NAME="agents: protect default branch"
PROJECT_BOARD_SERVICE=agent-gh-projects

die() { echo "agent-github: $*" >&2; exit 1; }

require_valid_name() {
    [[ $1 =~ ^[a-z0-9][a-z0-9._-]*$ ]] || die "invalid project name: $1"
    [[ -d $PROJECTS_DIR/$1 ]] || die "no such project: $PROJECTS_DIR/$1"
}

repository_of() {
    local remote
    remote=$(git -C "$PROJECTS_DIR/$1" remote get-url origin 2>/dev/null) || die "$1 has no origin remote"
    remote=${remote%.git}
    case $remote in
        https://github.com/*) echo "${remote#https://github.com/}" ;;
        git@github.com:*) echo "${remote#git@github.com:}" ;;
        *) die "the origin of $1 is not on github.com: $remote" ;;
    esac
}

stored_token() { secret-tool lookup service agent-gh project "$1" 2>/dev/null || true; }

stored_project_board_token() { secret-tool lookup service "$PROJECT_BOARD_SERVICE" 2>/dev/null || true; }

# The token travels through stdin (curl -H @-), never through argv where other processes can read it.
api_request_with_token() {
    local token=$1 path=$2 headers_file=$3 body_file=$4
    printf 'Authorization: Bearer %s\n' "$token" |
        curl -sS -o "$body_file" -D "$headers_file" -w '%{http_code}' -H @- \
            -H 'Accept: application/vnd.github+json' "https://api.github.com$path"
}

api_body_with_token() {
    local token=$1 path=$2
    printf 'Authorization: Bearer %s\n' "$token" |
        curl -sS -H @- -H 'Accept: application/vnd.github+json' "https://api.github.com$path"
}

graphql_body_with_token() {
    local token=$1 query=$2
    printf 'Authorization: Bearer %s\n' "$token" |
        curl -sS -H @- -H 'Content-Type: application/json' \
            --data "$(jq -n --arg query "$query" '{query: $query}')" https://api.github.com/graphql
}

header_value() {
    local name=$1 headers_file=$2
    awk -F': ' -v name="$name" 'tolower($1) == name { print $2 }' "$headers_file" | tr -d '\r'
}

# Every token can read public repositories, so isolation is judged on private ones only.
private_repositories_visible_to() {
    local token=$1 page=1 batch
    while :; do
        batch=$(api_body_with_token "$token" "/user/repos?per_page=100&page=$page")
        jq -r 'if type == "array" then .[] | select(.private) | .full_name else empty end' <<<"$batch"
        [[ $(jq 'if type == "array" then length else 0 end' <<<"$batch") -eq 100 ]] || break
        page=$((page + 1))
    done | sort | paste -sd' '
}

verify() {
    local name=$1 repository token headers_file body_file status expiration expected private_visible
    repository=$(repository_of "$name")
    token=$(stored_token "$name")
    if [[ -z $token ]]; then
        echo "$name: no token in the keyring"
        return 1
    fi
    headers_file=$(mktemp)
    body_file=$(mktemp)
    status=$(api_request_with_token "$token" "/repos/$repository" "$headers_file" "$body_file")
    expiration=$(header_value github-authentication-token-expiration "$headers_file")
    expected=$(jq -r 'if .private == true then .full_name else "" end' "$body_file" 2>/dev/null || true)
    rm -f "$headers_file" "$body_file"
    if [[ $status != 200 ]]; then
        echo "$name: the token cannot read $repository (HTTP $status)"
        return 1
    fi
    private_visible=$(private_repositories_visible_to "$token")
    if [[ $private_visible != "$expected" ]]; then
        echo "$name: the token can read private repositories other than $repository: $private_visible"
        return 1
    fi
    echo "$name: ok, sees no private repository except $repository, expires ${expiration:-never}"
}

# A classic token lists its scopes in X-OAuth-Scopes; anything beyond "project" would reach code.
verify_project_board_token() {
    local token headers_file body_file status scopes expiration projects
    token=$(stored_project_board_token)
    if [[ -z $token ]]; then
        echo "project boards: no token in the keyring"
        return 1
    fi
    headers_file=$(mktemp)
    body_file=$(mktemp)
    status=$(api_request_with_token "$token" /user "$headers_file" "$body_file")
    scopes=$(header_value x-oauth-scopes "$headers_file")
    expiration=$(header_value github-authentication-token-expiration "$headers_file")
    rm -f "$headers_file" "$body_file"
    if [[ $status != 200 ]]; then
        echo "project boards: the token is rejected (HTTP $status)"
        return 1
    fi
    if [[ $scopes != project ]]; then
        echo "project boards: the token has scopes \"$scopes\"; it must have project and nothing else"
        return 1
    fi
    projects=$(graphql_body_with_token "$token" '{ viewer { projectsV2(first: 1) { totalCount } } }' |
        jq -r '.data.viewer.projectsV2.totalCount // empty')
    if [[ -z $projects ]]; then
        echo "project boards: the token cannot list your projects"
        return 1
    fi
    echo "project boards: ok, scope project only, sees $projects project(s), expires ${expiration:-never}"
}

# Pre-fills everything GitHub accepts as URL parameters; only the repository choice stays manual.
fine_grained_token_url() {
    local name=$1 owner=$2
    jq -rn --arg base "$NEW_TOKEN_URL" --arg name "agent $name" --arg owner "$owner" \
        --arg description "Coding agents working on $name, passed in by the greywall launcher" '
        $base + "?" + ([
            "name=" + ($name | @uri),
            "description=" + ($description | @uri),
            "target_name=" + ($owner | @uri),
            "expires_in=90",
            "contents=write", "issues=write", "pull_requests=write", "workflows=write",
            "actions=read", "metadata=read"
        ] | join("&"))'
}

print_token_instructions() {
    local name=$1 repository=$2
    echo "The page that opens is pre-filled (GitHub has no API to create tokens). Check it shows:"
    echo "  Token name           agent $name"
    echo "  Expiration           90 days"
    echo "  Resource owner       ${repository%%/*}"
    echo "  Repository permissions"
    echo "    Read and write     Contents, Issues, Pull requests, Workflows"
    echo "    Read-only          Actions, Metadata"
    echo "  Account permissions  No access"
    echo "Set by hand:"
    echo "  Repository access    Only select repositories: $repository"
    echo "Generate the token, then paste it at the prompt below; it is not echoed."
}

token() {
    local name=$1 repository url
    require_valid_name "$name"
    repository=$(repository_of "$name")
    url=$(fine_grained_token_url "$name" "${repository%%/*}")
    print_token_instructions "$name" "$repository"
    xdg-open "$url" >/dev/null 2>&1 || echo "open $url"
    secret-tool store --label="GitHub token for $name agents" service agent-gh project "$name"
    verify "$name"
}

project_board_token() {
    local url="$NEW_CLASSIC_TOKEN_URL?scopes=project&description=agent-gh-projects"
    echo "The page that opens is pre-filled with the project scope only. Check that:"
    echo "  Note                 agent-gh-projects"
    echo "  Scopes               project ticked, every other box empty"
    echo "Choose an expiration (90 days is a good default), generate the token, then paste it at the"
    echo "prompt below; it is not echoed."
    xdg-open "$url" >/dev/null 2>&1 || echo "open $url"
    secret-tool store --label="GitHub Project boards token for agents" service "$PROJECT_BOARD_SERVICE"
    verify_project_board_token
}

check_cli_storage() {
    if grep -q oauth_token ~/.config/gh/hosts.yml 2>/dev/null; then
        echo "gh: your own token is in ~/.config/gh/hosts.yml, readable by sandboxed agents; log in again with keyring storage"
        return 1
    fi
    echo "gh: your own token is in the keyring, not in hosts.yml"
}

relogin() {
    echo "On the page that opens: Authorized OAuth Apps, GitHub CLI, Revoke."
    xdg-open "$AUTHORIZED_APPS_URL" >/dev/null 2>&1 || echo "open $AUTHORIZED_APPS_URL"
    read -r -p "Press Enter once it is revoked, to log in again... " _
    gh auth login
    check_cli_storage
}

ruleset_body() {
    jq -n --arg name "$RULESET_NAME" '{
        name: $name, target: "branch", enforcement: "active",
        conditions: {ref_name: {include: ["~DEFAULT_BRANCH"], exclude: []}},
        rules: [
            {type: "deletion"},
            {type: "non_fast_forward"},
            {type: "pull_request", parameters: {
                required_approving_review_count: 0, dismiss_stale_reviews_on_push: false,
                require_code_owner_review: false, require_last_push_approval: false,
                required_review_thread_resolution: false}}
        ]}'
}

protect() {
    local name=$1 repository existing
    require_valid_name "$name"
    repository=$(repository_of "$name")
    if ! existing=$(gh api "repos/$repository/rulesets" --jq ".[] | select(.name == \"$RULESET_NAME\") | .id" 2>&1); then
        die "cannot manage rulesets on $repository: $existing"
    fi
    if [[ -n $existing ]]; then
        ruleset_body | gh api -X PUT "repos/$repository/rulesets/$existing" --input - >/dev/null
        echo "$name: ruleset updated on $repository"
    else
        ruleset_body | gh api -X POST "repos/$repository/rulesets" --input - >/dev/null
        echo "$name: default branch of $repository protected"
    fi
}

check() {
    local project name
    check_cli_storage || true
    for project in "$PROJECTS_DIR"/*/; do
        name=$(basename "$project")
        [[ -n $(stored_token "$name") ]] || continue
        verify "$name" || true
    done
    if [[ -n $(stored_project_board_token) ]]; then
        verify_project_board_token || true
    fi
}

usage() {
    echo "usage: agent-github.sh token <project> | project-board-token | relogin | protect <project> | check" >&2
    exit 2
}

case ${1:-} in
    token) token "${2:?usage: agent-github.sh token <project>}" ;;
    project-board-token) project_board_token ;;
    relogin) relogin ;;
    protect) protect "${2:?usage: agent-github.sh protect <project>}" ;;
    check) check ;;
    *) usage ;;
esac
