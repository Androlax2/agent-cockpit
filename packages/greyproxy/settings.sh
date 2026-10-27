#!/usr/bin/env bash
# Keeps greyproxy's conversation tracking off (make after-apply), and fails loud when it cannot.
# Why: after each LLM request greyproxy 0.4.5's assembler LIKE-scans the request bodies of every
# stored LLM transaction for the session and parses all of them again. With 7 busy sessions that held
# greyproxy at 95% of a core and 700 MiB until the cockpit failsafe froze them (2026-10-27). Nothing
# in the cockpit reads the conversations; the traffic history stays.
# The setting goes through greyproxy's API, so it applies at once and greyproxy keeps it in its
# settings.json across restarts. GREYPROXY_API_URL overrides the API (tests).
set -euo pipefail

api=${GREYPROXY_API_URL:-http://127.0.0.1:43080/api}
token_file="${GREYPROXY_DATA_HOME:-${XDG_DATA_HOME:-$HOME/.local/share}/greyproxy}/api-token"
headers=()
if [[ -r $token_file ]]; then
    headers=(-H "Authorization: Bearer $(<"$token_file")")
fi

tracking() {
    curl -sSf --max-time 5 "${headers[@]}" "$api/settings" | python3 -c 'import json, sys; print(str(json.load(sys.stdin)["conversations"]["enabled"]).lower())'
}

state=$(tracking) || { echo "greyproxy-settings: cannot read $api/settings; is greyproxy running? Conversation tracking may still be on." >&2; exit 1; }
if [[ $state == false ]]; then
    echo "greyproxy conversation tracking is off"
    exit 0
fi
curl -sSf --max-time 5 "${headers[@]}" -X PUT -H 'Content-Type: application/json' \
    -d '{"conversations":{"enabled":false}}' "$api/settings" > /dev/null \
    || { echo "greyproxy-settings: greyproxy refused to turn conversation tracking off" >&2; exit 1; }
state=$(tracking) || state=unknown
if [[ $state != false ]]; then
    echo "greyproxy-settings: conversation tracking is still $state after turning it off" >&2
    exit 1
fi
echo "greyproxy conversation tracking turned off: it held greyproxy at a full core. The next make after-apply turns it off again if it is switched back on."
