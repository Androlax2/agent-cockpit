#!/usr/bin/env bash
# Codex owns the dedicated quota account and its token refresh lifecycle.
set -euo pipefail
umask 077

CACHE="${XDG_RUNTIME_DIR:-/tmp}/codex-usage.json"
ERROR_CACHE="${XDG_RUNTIME_DIR:-/tmp}/codex-usage-error.json"
HELPER="$(dirname -- "${BASH_SOURCE[0]}")/codex-quota.py"

if [[ ${1:-} == --login ]]; then
    python3 "$HELPER" --login
    bash "${BASH_SOURCE[0]}" --force
    if [[ -f $ERROR_CACHE ]]; then
        jq -r '.error' "$ERROR_CACHE" >&2
        exit 1
    fi
    exit 0
fi

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

if ! quota=$(python3 "$HELPER" 2>/dev/null); then
    fail "Codex quota unavailable; check Codex CLI or run codex-usage.sh --login"
fi
if ! jq -e '.rate_limit | type == "object"' <<< "$quota" >/dev/null 2>&1; then
    fail "Codex usage response unrecognized"
fi
jq '{rate_limit: .rate_limit}' <<< "$quota" > "$CACHE.tmp"
mv "$CACHE.tmp" "$CACHE"
rm -f "$ERROR_CACHE"
