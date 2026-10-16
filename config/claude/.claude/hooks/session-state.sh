#!/usr/bin/env bash
# Claude Code hooks → the OSD daemon (~/.config/osd/osd.py): one `session <json>` line per event with
# the session's state (working | waiting | needs-you | gone), its project, the focus target of its
# window (KITTY_* can be stale after a tmux re-attach; the daemon re-resolves it) and an optional
# banner message. The daemon keeps the registry, colours the Hyprland window
# border, feeds the waybar chip and shows the top-centre banners. A swaync toast is only the fallback
# when the OSD cannot be reached (daemon down, or a sandbox without the FIFO).
# Runs inside greywall: the session bus is exposed (notifications only) and $XDG_RUNTIME_DIR/osd.fifo
# must be in allowWrite.
set -euo pipefail

# A nested claude (a `claude -p` that a tool of another session runs) is not a session of yours:
# it shares the parent's pane and would shadow it in the manager. Its hooks have two claude
# processes among their ancestors instead of one, and report nothing.
claude_ancestors=0
pid=$$
while (( pid > 1 )); do
    pid=$(awk '/^PPid:/ { print $2 }' "/proc/$pid/status" 2>/dev/null) || break
    [[ -n $pid ]] || break
    [[ $(cat "/proc/$pid/comm" 2>/dev/null) == claude ]] && (( claude_ancestors++ )) || true
done
if (( claude_ancestors >= 2 )); then
    cat >/dev/null
    exit 0
fi

payload=$(cat)
event=$(jq -r '.hook_event_name' <<<"$payload")
session=$(jq -r '.session_id' <<<"$payload")
cwd=$(jq -r '.cwd // ""' <<<"$payload")
project=$(basename "$cwd")
fifo="${XDG_RUNTIME_DIR:-/tmp}/osd.fifo"
toast_id_file="$HOME/.cache/claude/toast-$session.id"

state=
task=
message=
sticky=false
case $event in
    SessionStart)
        state=waiting
        ;;
    UserPromptSubmit)
        state=working
        task=$(jq -r '.prompt // "" | split("\n")[0] | gsub("\\s+"; " ") | .[:60]' <<<"$payload")
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
        ;;
    SessionEnd)
        state=gone
        ;;
    *)
        echo "session-state.sh: unexpected hook event $event" >&2
        exit 1
        ;;
esac

report=$(jq -cn \
    --arg session "$session" --arg state "$state" --arg project "$project" --arg cwd "$cwd" \
    --arg task "$task" --arg message "$message" --argjson sticky "$sticky" \
    --arg kitty_pid "${KITTY_PID:-}" --arg kitty_window "${KITTY_WINDOW_ID:-}" \
    --arg kitty_listen "${KITTY_LISTEN_ON:-}" --arg tmux_pane "${TMUX_PANE:-}" \
    '{$session, $state, $project, $cwd, $task, $message, $sticky,
      $kitty_pid, $kitty_window, $kitty_listen, $tmux_pane}
     | with_entries(select(.value != "" and .value != false))')

# A FIFO without its daemon would block the open forever; give up after a second instead.
send_to_osd() {
    [[ -p $fifo ]] || return 1
    timeout 1 tee "$fifo" >/dev/null <<<"$1" && return 0
    echo "session-state.sh: osd fifo not read, is osd.py running?" >&2
    return 1
}

if send_to_osd "session $report"; then
    exit 0
fi

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
