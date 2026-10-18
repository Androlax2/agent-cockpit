#!/usr/bin/env bash
# Claude Code hooks → the OSD daemon (osd, apps/desktop): one `session <json>` line per event with
# the session's state (working | waiting | needs-you | gone), its project, the focus target of its
# window (KITTY_* can be stale after a tmux re-attach; the daemon re-resolves it) and an optional
# banner message. The daemon keeps the registry, colours the Hyprland window
# border, feeds the waybar chip and shows the top-centre banners. A swaync toast is only the fallback
# when the OSD cannot be reached (daemon down, or a sandbox without the FIFO).
# Runs inside greywall: the session bus is exposed (notifications only) and $XDG_RUNTIME_DIR/osd.fifo
# must be in allowWrite.
set -euo pipefail

# A nested claude (a `claude -p` that a tool of another session runs) is not a session of yours:
# it shares the parent's pane and would shadow it in the manager, so its hooks report nothing.
# Claude Code marks every process a tool runs with CLAUDECODE=1; a session started by you from a
# shell has no such mark, a nested one carries it in its own environment. Counting processes
# named claude does not work since 2.1.300: the launcher, the daemon and the pty host are all
# ancestors of every session. The marked ancestor is looked for among Claude processes only
# (comm claude or a version number), so the hook's own tool shells never count.
is_claude_process() { # pid
    local comm
    comm=$(cat "/proc/$1/comm" 2>/dev/null) || return 1
    [[ $comm == claude || $comm =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]
}
nested=0
pid=$$
while (( pid > 1 )); do
    pid=$(awk '/^PPid:/ { print $2 }' "/proc/$pid/status" 2>/dev/null) || break
    [[ -n $pid ]] || break
    if is_claude_process "$pid" && tr '\0' '\n' < "/proc/$pid/environ" 2>/dev/null | grep -qx 'CLAUDECODE=1'; then
        nested=1
        break
    fi
done
if (( nested )); then
    cat >/dev/null
    exit 0
fi

# The session binary runs under Claude's daemon with a scrubbed environment (2.1.300+): the tmux
# pane and the kitty window the session was started in are only known to the launcher above it.
# Take them from the nearest ancestor that has them when this process does not.
inherited() { # NAME
    local pid=$$ value
    while (( pid > 1 )); do
        pid=$(awk '/^PPid:/ { print $2 }' "/proc/$pid/status" 2>/dev/null) || break
        [[ -n $pid ]] && (( pid > 1 )) || break
        value=$(tr '\0' '\n' 2>/dev/null < "/proc/$pid/environ" | sed -n "s/^$1=//p" | head -1)
        if [[ -n $value ]]; then
            printf '%s' "$value"
            return
        fi
    done
}
TMUX_PANE=${TMUX_PANE:-$(inherited TMUX_PANE)}
KITTY_PID=${KITTY_PID:-$(inherited KITTY_PID)}
KITTY_WINDOW_ID=${KITTY_WINDOW_ID:-$(inherited KITTY_WINDOW_ID)}
KITTY_LISTEN_ON=${KITTY_LISTEN_ON:-$(inherited KITTY_LISTEN_ON)}

payload=$(cat)
event=$(jq -r '.hook_event_name' <<<"$payload")
session=$(jq -r '.session_id' <<<"$payload")
cwd=$(jq -r '.cwd // ""' <<<"$payload")
project=$(basename "$cwd")
fifo="${XDG_RUNTIME_DIR:-/tmp}/osd.fifo"
toast_id_file="$HOME/.cache/claude/toast-$session.id"

# What a turn left in the repository, worked out here, inside the session's sandbox: git reads the
# repository's own configuration, which the session can write, so it must never run outside.
turn_head_file="$HOME/.cache/claude/turn-$session.head"
outcome_of_turn() { # cwd: commits since the turn's start, uncommitted changes, or "no changes"
    git -C "$1" rev-parse --git-dir >/dev/null 2>&1 || return 0
    local start count stat files insertions deletions summary=""
    start=$(cat "$turn_head_file" 2>/dev/null || true)
    count=0
    if [[ -n $start ]]; then count=$(git -C "$1" rev-list --count "$start..HEAD" 2>/dev/null || echo 0); fi
    if (( count == 1 )); then summary="1 commit"; elif (( count > 1 )); then summary="$count commits"; fi
    stat=$(git -C "$1" diff --shortstat HEAD 2>/dev/null || true)
    files=$(grep -oE '[0-9]+ files? changed' <<<"$stat" | grep -oE '^[0-9]+' || echo 0)
    insertions=$(grep -oE '[0-9]+ insertions?' <<<"$stat" | grep -oE '^[0-9]+' || echo 0)
    deletions=$(grep -oE '[0-9]+ deletions?' <<<"$stat" | grep -oE '^[0-9]+' || echo 0)
    if (( files > 0 )); then
        local noun=files
        if (( files == 1 )); then noun=file; fi
        summary+="${summary:+ · }$files $noun changed, +$insertions −$deletions uncommitted"
    fi
    echo "${summary:-no changes}"
}

state=
task=
message=
outcome=
sticky=false
case $event in
    SessionStart)
        state=waiting
        ;;
    UserPromptSubmit)
        state=working
        # The commit the turn starts from, to tell what it did when it stops.
        { mkdir -p "$(dirname "$turn_head_file")"; git -C "$cwd" rev-parse HEAD > "$turn_head_file"; } 2>/dev/null || rm -f "$turn_head_file"
        # Only what you typed names the task: prompts from the system (task notifications, loop
        # wake-ups) say nothing about it.
        if [[ $(jq -r '.source // "user"' <<<"$payload") == user ]]; then
            task=$(jq -r '.prompt // "" | split("\n")[0] | gsub("\\s+"; " ") | .[:60]' <<<"$payload")
        fi
        ;;
    PostToolUse)
        state=working
        ;;
    Notification)
        message=$(jq -r '.message | gsub("\\s+"; " ")' <<<"$payload")
        # Requests for a human stick and turn the session red; turn-end notices leave it waiting;
        # anything informational (auth, quota, computer use, plugins) shows its message and keeps the state.
        case $(jq -r '.notification_type // ""' <<<"$payload") in
            permission_prompt | worker_permission_prompt | elicitation_dialog | elicitation_url_dialog | agent_needs_input)
                state=needs-you; sticky=true ;;
            idle_prompt) state=waiting; sticky=true ;;
            agent_completed | push_notification) state=waiting ;;
            *) state= ;;
        esac
        ;;
    Stop)
        state=waiting
        message="Finished, waiting for you"
        outcome=$(outcome_of_turn "$cwd")
        ;;
    SessionEnd)
        state=gone
        rm -f "$turn_head_file"
        ;;
    *)
        echo "session-state.sh: unexpected hook event $event" >&2
        exit 1
        ;;
esac

report=$(jq -cn \
    --arg session "$session" --arg state "$state" --arg project "$project" --arg cwd "$cwd" \
    --arg task "$task" --arg message "$message" --arg outcome "$outcome" --argjson sticky "$sticky" \
    --arg kitty_pid "${KITTY_PID:-}" --arg kitty_window "${KITTY_WINDOW_ID:-}" \
    --arg kitty_listen "${KITTY_LISTEN_ON:-}" --arg tmux_pane "${TMUX_PANE:-}" \
    '{$session, $state, $project, $cwd, $task, $message, $outcome, $sticky,
      $kitty_pid, $kitty_window, $kitty_listen, $tmux_pane}
     | with_entries(select(.value != "" and .value != false))')

# One line per event in ~/.cache/claude/hook.log, the one place the sandbox lets the hook write:
# the event, the session, the state and where the report went, never prompt text. 512 KB, then
# one previous file. `claude-sessions logs` shows the end of it.
hook_log() { # message
    local log="$HOME/.cache/claude/hook.log"
    {
        mkdir -p "$(dirname "$log")"
        if (( $(stat -c %s "$log" 2>/dev/null || echo 0) > 524288 )); then mv -f "$log" "$log.1"; fi
        printf '%s %s %s %s: %s\n' "$(date -Iseconds)" "$event" "${session:0:8}" "${state:-·}" "$1" >> "$log"
    } 2>/dev/null || true
}

# A FIFO without its daemon would block the open forever; give up after a second instead.
send_to_osd() {
    [[ -p $fifo ]] || return 1
    timeout 1 tee "$fifo" >/dev/null <<<"$1" && return 0
    echo "session-state.sh: osd fifo not read, is osd running?" >&2
    return 1
}

if send_to_osd "session $report"; then
    hook_log "sent to the osd daemon"
    exit 0
fi
hook_log "osd daemon not reading $fifo: falling back to a toast"

# Fallback: swaync toasts, closed again once the session reports without a message.
if [[ -n $message ]]; then
    mkdir -p "$(dirname "$toast_id_file")"
    urgency=normal
    [[ $sticky == true ]] && urgency=critical
    notify-send --print-id -a "Claude Code" -u "$urgency" "Claude · $project" "$message" > "$toast_id_file"
elif [[ -f $toast_id_file ]]; then
    toast_id=$(<"$toast_id_file")
    rm -f "$toast_id_file"
    gdbus call --session --dest org.freedesktop.Notifications \
        --object-path /org/freedesktop/Notifications \
        --method org.freedesktop.Notifications.CloseNotification "$toast_id" >/dev/null
fi
