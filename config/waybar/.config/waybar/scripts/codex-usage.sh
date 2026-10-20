#!/usr/bin/env bash
# Read-only quota polling: the owning client, not this script, refreshes OAuth credentials.
set -euo pipefail
umask 077

CACHE="${XDG_RUNTIME_DIR:-/tmp}/codex-usage.json"
ERROR_CACHE="${XDG_RUNTIME_DIR:-/tmp}/codex-usage-error.json"
AUTH="${XDG_DATA_HOME:-$HOME/.local/share}/opencode/auth.json"
DATABASE="${OPENCODE_DB:-${XDG_DATA_HOME:-$HOME/.local/share}/opencode/opencode.db}"
CODEX_AUTH="${CODEX_HOME:-$HOME/.codex}/auth.json"

exec 9>"$CACHE.lock"
flock 9
if [[ ${1:-} != --force && -f $CACHE ]] && (( $(date +%s) - $(stat -c %Y "$CACHE") < 240 )); then
    exit 0
fi

fail() {
    jq -cn --arg reason "$1" '{error: $reason}' > "$ERROR_CACHE.tmp"
    mv "$ERROR_CACHE.tmp" "$ERROR_CACHE"
    exit 0
}

token=""
account=""
auth=null
is_v2=false
if [[ -e $DATABASE ]]; then
    has_credentials=$(sqlite3 -readonly -init /dev/null -cmd '.timeout 1000' "$DATABASE" \
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='credential'" 2>/dev/null) || fail "OpenCode credential database unavailable"
    if [[ $has_credentials == 1 ]]; then
        is_v2=true
        # Match V2's selection: an explicit active flag, otherwise newest creation time then id.
        auth=$(sqlite3 -readonly -init /dev/null -cmd '.timeout 1000' "$DATABASE" \
            "SELECT value FROM credential WHERE integration_id='openai' ORDER BY active DESC, time_created DESC, id DESC LIMIT 1" \
            2>/dev/null) || fail "OpenCode credential database unavailable"
        auth=${auth:-null}
    fi
fi
if [[ $is_v2 == false && -r $AUTH ]]; then
    auth=$(jq -c '.openai // null' "$AUTH" 2>/dev/null) || fail "OpenCode credentials unavailable"
fi
if ! jq -e 'type == "object" or . == null' <<< "$auth" >/dev/null 2>&1; then
    fail "OpenCode credentials unrecognized"
fi
if [[ $(jq -r '.type // empty' <<< "$auth") == oauth ]]; then
    token=$(jq -r '.access // empty' <<< "$auth")
    account=$(jq -r '.metadata.accountID // .accountId // empty' <<< "$auth")
    [[ -n $token ]] || fail "OpenCode OAuth token unavailable"
    if ! jq -e --argjson now "$(date +%s%3N)" '(.expires | type == "number") and .expires > $now' <<< "$auth" >/dev/null; then
        fail "OpenCode OAuth expired; use OpenCode to refresh it"
    fi
elif [[ $is_v2 == true && $auth != null ]]; then
    fail "Active OpenAI account is not a subscription sign-in"
fi
if [[ -z $token ]]; then
    [[ -r $CODEX_AUTH ]] || fail "No Codex subscription sign-in"
    token=$(jq -r '.tokens.access_token // empty' "$CODEX_AUTH" 2>/dev/null) || fail "Codex credentials unavailable"
    account=$(jq -r '.tokens.account_id // empty' "$CODEX_AUTH" 2>/dev/null) || fail "Codex credentials unavailable"
fi
[[ -n $token ]] || fail "No Codex OAuth token"
# Header values come from files: reject line breaks before feeding curl's header parser.
[[ $token != *$'\n'* && $token != *$'\r'* && $account != *$'\n'* && $account != *$'\r'* ]] || fail "Invalid OAuth credentials"

body=$(mktemp "${CACHE}.XXXXXX")
trap 'rm -f "$body"' EXIT
if ! {
    printf 'Authorization: Bearer %s\n' "$token"
    [[ -z $account ]] || printf 'ChatGPT-Account-Id: %s\n' "$account"
    printf 'Accept: application/json\n'
} | curl -sS -f -m 15 --header @- --user-agent codex-cli \
        https://chatgpt.com/backend-api/wham/usage > "$body" 2>/dev/null; then
    fail "Codex usage request failed"
fi
if ! jq -e '.rate_limit | type == "object"' "$body" >/dev/null 2>&1; then
    fail "Codex usage response unrecognized"
fi
# Keep only quota fields, never unrelated account data from a changing upstream response.
jq '{rate_limit: .rate_limit}' "$body" > "$CACHE.tmp"
mv "$CACHE.tmp" "$CACHE"
rm -f "$ERROR_CACHE"
