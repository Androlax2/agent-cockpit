#!/usr/bin/env bash
# Waybar custom module: running Claude Code sessions as "● 2  ○ 1  ! 1" (working / waiting / needs you),
# a dim robot glyph when none runs (it is the last module of its island, which must not show empty). ~/.config/osd/osd.py writes the snapshot and sends RTMIN+16 on every change;
# the 60 s interval only refreshes the ages in the tooltip. Click (osd-show focus-next) jumps to the
# most urgent session. CLAUDE_SESSIONS_SNAPSHOT overrides the path for testing against a fixture.
SNAPSHOT="${CLAUDE_SESSIONS_SNAPSHOT:-${XDG_RUNTIME_DIR:-/tmp}/claude-sessions.json}"
IDLE_GLYPH=$'\U000F06A9'  # nf-md-robot, the Claude workspace icon

print_idle() {
    jq -cn --arg glyph "$IDLE_GLYPH" '{text: $glyph, tooltip: "No Claude session\nClick: open the manager", class: "idle", alt: "idle"}'
}

if [[ ! -s $SNAPSHOT ]]; then
    print_idle
    exit 0
fi

jq -c --argjson now "$(date +%s)" --arg idle_glyph "$IDLE_GLYPH" '
def pango: tostring | gsub("&"; "&amp;") | gsub("<"; "&lt;") | gsub(">"; "&gt;");
def age: ($now - .since) | if . < 60 then "\(.)s" elif . < 3600 then "\(. / 60 | floor)m" else "\(. / 3600 | floor)h" end;
def colour: {"working": "#7aa2f7", "waiting": "#e0af68", "needs-you": "#f7768e"}[.];
def glyph: {"working": "●", "waiting": "○", "needs-you": "!"}[.];
def count($state): [.sessions[] | select(.state == $state)] | length;
def chip($state): count($state) as $n | if $n > 0 then "<span color=\"\($state | colour)\">\($state | glyph) \($n)</span>" else empty end;
def most_urgent: if count("needs-you") > 0 then "needs-you" elif count("waiting") > 0 then "waiting" else "working" end;

if (.sessions | length) == 0 then {text: $idle_glyph, tooltip: "No Claude session\nClick: open the manager", class: "idle", alt: "idle"} else
{
  text: ([chip("working"), chip("waiting"), chip("needs-you")] | join("  ")),
  tooltip: ([
      "<span color=\"#7aa2f7\"><b>Claude sessions</b></span>",
      (.sessions[] | "<span color=\"\(.state | colour)\">\(.state | glyph)</span>  <b>\(.project | pango)</b>  \(.task // "" | pango)  <span color=\"#565f89\">\(age)</span>"),
      "",
      "<span color=\"#565f89\">Click: focus the most urgent session</span>"
    ] | join("\n")),
  class: most_urgent,
  alt: most_urgent
} end' "$SNAPSHOT"
