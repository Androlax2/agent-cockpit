#!/usr/bin/env bash
# Claude Code hooks: a top-centre OSD banner (~/.config/osd), with a swaync toast only as the
# fallback when the OSD cannot be reached (daemon down, or a sandbox without the FIFO).
#   Notification       sticky banner (critical toast): Claude needs you
#   Stop               6 s banner (normal toast): the turn is over
#   UserPromptSubmit, PostToolUse, SessionEnd
#                      you acted (or the session ended): drop the banner, close a fallback toast
# The built-in kitty channel dies inside tmux (allow-passthrough is off), so this is the only
# notification path. It runs inside greywall, which exposes the session bus (notifications only)
# and must have $XDG_RUNTIME_DIR/osd.fifo in allowWrite for the banner to work.
set -euo pipefail

payload=$(cat)
event=$(jq -r '.hook_event_name' <<<"$payload")
session=$(jq -r '.session_id' <<<"$payload")
project=$(basename "$(jq -r '.cwd // ""' <<<"$payload")")
title="Claude · $project"
fifo="${XDG_RUNTIME_DIR:-/tmp}/osd.fifo"
toast_id_file="$HOME/.cache/claude/toast-$session.id"

# A FIFO without its daemon would block the open forever; give up after a second instead.
# Fails when the OSD is unreachable so the caller can fall back to a toast.
send_to_osd() {
    [[ -p $fifo ]] || return 1
    timeout 1 tee "$fifo" >/dev/null <<<"$1" && return 0
    echo "notify.sh: osd fifo not read, is osd.py running?" >&2
    return 1
}

raise_banner() { # sticky(true|false) message
    local command
    command=$(jq -cn \
        --arg session "$session" --argjson sticky "$1" --arg title "$title" --arg message "$2" \
        --arg kitty_pid "${KITTY_PID:-}" --arg kitty_window "${KITTY_WINDOW_ID:-}" \
        --arg kitty_listen "${KITTY_LISTEN_ON:-}" --arg tmux_pane "${TMUX_PANE:-}" \
        '{$session, $sticky, $title, $message, $kitty_pid, $kitty_window, $kitty_listen, $tmux_pane}
         | with_entries(select(.value != ""))')
    send_to_osd "banner $command"
}

show_toast() { # urgency message
    mkdir -p "$(dirname "$toast_id_file")"
    notify-send --print-id -a "Claude Code" -u "$1" "$title" "$2" > "$toast_id_file"
}

notify() { # sticky(true|false) urgency message
    raise_banner "$1" "$3" || show_toast "$2" "$3"
}

clear_all() {
    send_to_osd "banner-clear $session" || true
    [[ -f $toast_id_file ]] || return 0
    local toast_id
    toast_id=$(<"$toast_id_file")
    rm -f "$toast_id_file"
    gdbus call --session --dest org.freedesktop.Notifications \
        --object-path /org/freedesktop/Notifications \
        --method org.freedesktop.Notifications.CloseNotification "$toast_id" >/dev/null
}

case $event in
    Notification)
        message=$(jq -r '.message | gsub("\\s+"; " ")' <<<"$payload")
        kind=$(jq -r '.notification_type // ""' <<<"$payload")
        if [[ $kind == auth_success ]]; then
            notify false normal "$message"
        else
            notify true critical "$message"
        fi
        ;;
    Stop)
        notify false normal "Finished, waiting for you"
        ;;
    UserPromptSubmit | PostToolUse | SessionEnd)
        clear_all
        ;;
    *)
        echo "notify.sh: unexpected hook event $event" >&2
        exit 1
        ;;
esac
