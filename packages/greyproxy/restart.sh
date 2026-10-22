#!/usr/bin/env bash
# Restarts greyproxy on the binary greyproxy-patch just installed, and fails, saying how to roll back
# to greyproxy.previous, unless it answers its health check within 10 s (build.sh runs it).
# GREYPROXY_HEALTH_URL and GREYPROXY_HEALTH_WAIT_S override the check (tests).
set -euo pipefail

target="$HOME/.local/bin/greyproxy"
health_url=${GREYPROXY_HEALTH_URL:-http://127.0.0.1:43080/api/health}
health_wait_s=${GREYPROXY_HEALTH_WAIT_S:-10}
rollback="cp --preserve=mode $target.previous $target && systemctl --user restart greyproxy"

systemctl --user restart greyproxy || { echo "greyproxy-patch: the restart failed. Roll back: $rollback" >&2; exit 1; }
for (( second = 0; second < health_wait_s; second++ )); do
    if [[ $(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "$health_url") == 200 ]]; then
        echo "greyproxy-patch: greyproxy answers its health check on the new build."
        exit 0
    fi
    sleep 1
done
echo "greyproxy-patch: greyproxy did not answer $health_url within $health_wait_s s. Roll back: $rollback" >&2
exit 1
