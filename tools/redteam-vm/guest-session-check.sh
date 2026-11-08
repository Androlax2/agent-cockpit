#!/usr/bin/env bash
# Runs inside the guest as "redteam", from "redteam-vm.sh check", after the isolation check.
# It proves a sandboxed claude session reaches its first report with nobody at its terminal
# (#1178): Claude Code reports only once past its own startup screens, and a session stopped on
# one shows `dialog=` in `torlek agent reports`. Starting the session needs the provider
# credential the owner drops with put-credential. Without one the step says NOT RUN and does
# not fail: `check` runs on a fresh guest, before any credential, as the isolation check
# (#1172), and its credential line already says `absent`.
# The session gets no prompt, so it runs no turn. Sent as one argument: commands get /dev/null.
set -uo pipefail

export PATH=$HOME/.local/bin:$PATH

project=$HOME/redteam-project
credentials=$HOME/.config/torlek/provider-credentials/claude
deadline_seconds=${TORLEK_REDTEAM_REPORT_DEADLINE:-90}
poll_seconds=3

if [[ -z $(find "$credentials" -maxdepth 1 -type f -size +0c 2>/dev/null) ]]; then
    echo "sandboxed session: NOT RUN: no claude credential in the guest, so nothing was tested (run put-credential, then check again)"
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
    # A screen the session cannot pass shows as a dialog, or as a question when screen reading
    # takes it for one: with no prompt sent, the agent has nothing of its own to ask.
    if [[ $report == *" dialog="* || $report == *" needs-input="* ]]; then
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
