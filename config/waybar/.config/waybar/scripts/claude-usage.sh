#!/usr/bin/env bash
# Claude subscription usage, one window per invocation, as a waybar-style JSON object:
#   claude-usage.sh five_hour    the current 5-hour window (clock glyph)
#   claude-usage.sh seven_day    the 7-day window (calendar glyph)
# The text is a dim glyph and the window's utilisation in percent, the class its own threshold
# (warning / critical); the tooltip lists every window the server reports. Read from the endpoint
# Claude Code's /usage uses (api.anthropic.com/api/oauth/usage, undocumented, so a schema change
# shows up as itself in the tooltip rather than being hidden). The response is cached in
# $XDG_RUNTIME_DIR/claude-usage.json, which the claude-sessions sidebar reads for its header; the
# sidebar runs this script every 5 min to keep that cache fresh.
# The OAuth token comes from ~/.claude/.credentials.json and is never refreshed here: Claude Code
# owns it, and a refresh from two places invalidates one of them. An expired token is shown as
# "stale" until a Claude session refreshes it.
# CLAUDE_USAGE_FIXTURE points at a saved response for testing without the network.
WINDOW=${1:?usage: claude-usage.sh five_hour|seven_day}
CREDENTIALS="$HOME/.claude/.credentials.json"
USAGE_URL="https://api.anthropic.com/api/oauth/usage"
CACHE="${XDG_RUNTIME_DIR:-/tmp}/claude-usage.json"
CACHE_MAX_AGE_S=240
WARNING_AT=70
CRITICAL_AT=90
# Material clock-outline (U+F0150) and calendar (U+F00ED) as UTF-8 bytes: $'\U…' needs a UTF-8 locale.
declare -A GLYPHS=([five_hour]=$(printf '\xf3\xb0\x85\x90') [seven_day]=$(printf '\xf3\xb0\x83\xad'))
GLYPH_COLOR="#a9b1d6"

print_unavailable() { # class reason
    jq -cn --arg class "$1" --arg reason "$2" \
        '{text: "–", tooltip: "<b>Claude usage</b>\n\($reason | gsub("&"; "&amp;") | gsub("<"; "&lt;") | gsub(">"; "&gt;"))", class: $class, alt: $class}'
}

cache_is_fresh() {
    [[ -f $CACHE ]] && (( $(date +%s) - $(stat -c %Y "$CACHE") < CACHE_MAX_AGE_S ))
}

fetch_response() {
    local token expires_at_ms
    token=$(jq -r '.claudeAiOauth.accessToken // empty' "$CREDENTIALS")
    expires_at_ms=$(jq -r '.claudeAiOauth.expiresAt // 0' "$CREDENTIALS")
    if [[ -z $token ]]; then
        print_unavailable unknown "no OAuth token in credentials (log in with claude)"
        return 1
    fi
    if (( expires_at_ms <= $(date +%s%3N) )); then
        print_unavailable stale "OAuth token expired; start a Claude session to refresh it"
        return 1
    fi
    local body
    if ! body=$(curl -sS -f -m 15 \
            -H "Authorization: Bearer $token" \
            -H "anthropic-beta: oauth-2025-04-20" \
            -H "Accept: application/json" \
            "$USAGE_URL" 2>"$errors"); then
        print_unavailable unknown "request failed: $(head -c 200 "$errors")"
        return 1
    fi
    printf '%s\n' "$body" > "$CACHE.tmp" && mv "$CACHE.tmp" "$CACHE"
}

errors=$(mktemp)
trap 'rm -f "$errors"' EXIT

plan="unknown plan"
if [[ -n ${CLAUDE_USAGE_FIXTURE:-} ]]; then
    response=$(cat "$CLAUDE_USAGE_FIXTURE")
else
    if [[ ! -r $CREDENTIALS ]]; then
        print_unavailable unknown "no credentials at $CREDENTIALS"
        exit 0
    fi
    plan=$(jq -r '.claudeAiOauth.subscriptionType // "unknown plan"' "$CREDENTIALS")
    # Both modules poll on the same schedule; the lock makes the second one read what the first fetched.
    # Without a writable runtime dir there is no cache either, so just fetch unlocked.
    if { exec 9>"$CACHE.lock"; } 2>/dev/null; then
        flock 9
    fi
    if ! cache_is_fresh && ! fetch_response; then
        exit 0
    fi
    response=$(cat "$CACHE")
fi

read -r -d '' PROGRAM <<'JQ'
def pango: gsub("&"; "&amp;") | gsub("<"; "&lt;") | gsub(">"; "&gt;");
def header: "<span color='#7aa2f7'><b>\(.)</b></span>";
# resets_at arrives as ISO 8601 with an optional fraction and either Z or an offset;
# fromdateiso8601 takes neither, so the offset is applied by hand.
def epoch:
  (capture("^(?<base>[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2})(\\.[0-9]+)?(?<zone>Z|(?<sign>[+-])(?<hours>[0-9]{2}):?(?<minutes>[0-9]{2}))$")
    // error("unrecognised timestamp \(.)")) as $parts
  | ("\($parts.base)Z" | fromdateiso8601)
    - (if $parts.zone == "Z" then 0
       else ($parts.hours | tonumber) * 3600 + ($parts.minutes | tonumber) * 60
            | if $parts.sign == "-" then -. else . end
       end);
def resets_in: if . == null then "" else (epoch | if . - $now < 86400 then strflocaltime("%H:%M") else strflocaltime("%a %H:%M") end) end;
def pct: if . == null then null else round end;
def bar: if . == null then "–" else "\(.)%" end;
def level_class: if . == null then "unknown" elif . >= $critical_at then "critical" elif . >= $warning_at then "warning" else "ok" end;
def window_line($label):
  if . == null then empty
  else "    \($label | . + (" " * (14 - length)))\(.utilization | pct | bar | . + (" " * (5 - length)))" +
       (if .resets_at then "<span color='#565f89'>resets \(.resets_at | resets_in)</span>" else "" end)
  end;

(.[$window].utilization | pct) as $value
| ($value | level_class) as $class
| {
    text: "<span color='\($glyph_color)'>\($glyph)</span> \($value | bar)",
    tooltip: ([
        ("Claude usage  \($plan | pango)" | header),
        (.five_hour | window_line("5-hour")),
        (.seven_day | window_line("7-day")),
        (.seven_day_opus | window_line("7-day Opus")),
        (.seven_day_sonnet | window_line("7-day Sonnet")),
        ((.model_scoped // [])[] | window_line("7-day \(.display_name // "model")")),
        "",
        "<span color='#565f89'>Click: usage page on claude.ai</span>"
      ] | join("\n")),
    class: $class,
    alt: $class
  }
JQ

if ! output=$(jq -c --arg window "$WINDOW" --arg plan "$plan" --argjson now "$(date +%s)" \
        --arg glyph "${GLYPHS[$WINDOW]}" --arg glyph_color "$GLYPH_COLOR" \
        --argjson warning_at "$WARNING_AT" --argjson critical_at "$CRITICAL_AT" \
        "$PROGRAM" <<<"$response" 2>"$errors"); then
    print_unavailable unknown "unexpected response: $(head -c 300 "$errors")"
    exit 0
fi
printf '%s\n' "$output"
