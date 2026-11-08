#!/usr/bin/env bash
# Runs inside the guest as "redteam", from "redteam-vm.sh check", after the isolation check.
# It proves a sandboxed claude session reaches its first report with nobody at its terminal
# (#1178): Claude Code reports only once past its own startup screens, and a session stopped on
# one shows `dialog=` in `torlek agent reports`. Starting the session needs the provider
# credential the owner drops with put-credential; without one the step says it did not run.
# The session gets no prompt, so it runs no turn. Sent as one argument: commands get /dev/null.
set -uo pipefail

export PATH=$HOME/.local/bin:$PATH

project=$HOME/redteam-project
credentials=$HOME/.config/torlek/provider-credentials/claude
deadline_seconds=${TORLEK_REDTEAM_REPORT_DEADLINE:-90}
poll_seconds=3

if [[ -z $(find "$credentials" -maxdepth 1 -type f -size +0c 2>/dev/null) ]]; then
    echo "sandboxed session: NOT CHECKED: no claude credential in the guest (run put-credential, then check again)"
    exit 0
fi

started=$(torlek session new "$project" --agent claude </dev/null 2>&1)
session=$(awk -F'\t' '$1 ~ /^s[0-9a-f]{12}$/ { print $1; exit }' <<<"$started")
if [[ -z $session ]]; then
    echo "FAIL: the sandboxed session did not start: $started"
    exit 1
fi
trap 'torlek session end "$session" </dev/null >/dev/null 2>&1' EXIT

report=""
for ((waited = 0; waited < deadline_seconds; waited += poll_seconds)); do
    report=$(torlek agent reports </dev/null 2>/dev/null | awk -v id="$session" '$1 == id')
    if [[ $report == *" dialog="* ]]; then
        echo "FAIL: sandboxed session $session waits on a screen: $report"
        exit 1
    fi
    state=$(awk '{ print $2 }' <<<"$report")
    if [[ -n $state && $state != - ]]; then
        echo "sandboxed session: reached its first report: $report"
        exit 0
    fi
    sleep "$poll_seconds"
done
echo "FAIL: sandboxed session $session did not report within ${deadline_seconds}s: ${report:-no report line}"
exit 1
