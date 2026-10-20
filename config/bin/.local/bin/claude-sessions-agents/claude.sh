# Claude Code, as claude-sessions runs it (sourced by claude-sessions and claude-sessions-left).
# The verbs every agent defines are listed in claude-sessions (AGENT_VERBS); README.md in this
# folder says what each one does. These call the manager's helpers (loading_overlay, view_session,
# pane_at_prompt, …), which exist by the time a verb runs.
# shellcheck shell=bash

agent_claude_label() { echo "Claude Code"; }

# A UUID, which Claude reports from its first hook.
agent_claude_is_session_id() { # id
    [[ $1 =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]]
}

# claude-auto-retry's tmux sessions, and the manager's own launches (claude-retry-bg-*,
# claude-retry-restart-*): every claude-retry-* that no other agent's longer prefix claims.
agent_claude_tmux_prefix() { echo "claude-retry-"; }

agent_claude_can() { # feature
    case $1 in
        effort|billing|compact|remote|redraw) return 0 ;;
        *) return 1 ;;
    esac
}

# Every `fish -ic '…' -- args` keeps the --: without it fish takes an argument such as --resume or
# --effort as its own option and exits ("unknown option") before Claude starts.
# Appended to every launch in the viewer pane: a start that fails says so and stays on screen,
# instead of leaving a bare "Pane is dead (status 1)". Claude itself runs in its own tmux session
# (claude-auto-retry), so its error may be there; the epilogue lists those sessions.
CLAUDE_LAUNCH_EPILOGUE='; set -l code $status; if test $code -ne 0; claude-sessions log manager ERROR "claude exited with status $code in $PWD" 2>/dev/null; echo; echo "  ✗ claude exited with status $code"; echo "  Claude sessions still open:"; tmux ls 2>/dev/null | string match "*claude-retry*" | string replace -r "^" "    "; echo "  More: claude-sessions doctor · journalctl --user -u claude-sessions"; read -n 1 -P "  any key closes this pane " answer; end'

# A session started out of the viewer pane, in a tmux session named like claude-auto-retry's so the
# manager treats it as any other; inside tmux the launcher runs claude right there. A failed start
# is logged, since nobody watches that session.
# TMUX_PANE stays: tmux sets it to the new session's pane and the session reports it, so its card
# matches its tmux session (stripped, the card had no pane and the tmux session showed up as a
# second, unknown card).
claude_launch_detached() { # dir claude-arguments...
    local directory=$1 size
    shift
    require_plain_directory "$directory"
    size=$(detached_size)
    tmux new-session -d -s "claude-retry-restart-$(date +%s%N)" -x "${size% *}" -y "${size#* }" -c "$directory" \
        env CLAUDE_SESSIONS_INSIDE=1 CLAUDE_SESSIONS_CREDITS="$launch_credits" \
        fish -ic 'claude $argv; or claude-sessions log manager ERROR "claude exited with status $status in $PWD"' -- "$@"
}

# fish owns the `claude` function (greywall + claude-auto-retry). With no tmux variable left in
# the environment the launcher creates the session's own tmux session and attaches it here;
# with one still set it would run claude directly in this pane, which the next view kills.
# CLAUDE_SESSIONS_INSIDE stops the function from delegating back to this manager.
# The launch takes a few seconds (fish, the launcher, greywall, Claude): say so in the pane
# until Claude's own screen takes over.
agent_claude_new() { # dir [claude arguments]
    local directory=$1
    shift
    loading_overlay "Starting Claude in $(basename "$directory") …" "$directory"
    tmux respawn-pane -k -t "$(viewer_pane)" -c "$directory" \
        env -u TMUX -u TMUX_PANE CLAUDE_SESSIONS_INSIDE=1 CLAUDE_SESSIONS_CREDITS="$launch_credits" \
        fish -ic 'echo; echo "  ⟳ Starting Claude in $PWD …"; echo; claude $argv'"$CLAUDE_LAUNCH_EPILOGUE" -- "$@"
    : > "$CURRENT"
    label_panes "$(basename "$directory")"
}

# Claude takes its first message as its first argument.
agent_claude_new_with_prompt() { # dir prompt
    agent_claude_new "$1" "$2"
}

agent_claude_background() { # dir prompt
    tmux new-session -d -s "claude-retry-bg-$(date +%s)" -x 200 -y 50 -c "$1" \
        env CLAUDE_SESSIONS_INSIDE=1 CLAUDE_SESSIONS_CREDITS="$launch_credits" fish -ic 'claude $argv' -- "$2"
}

agent_claude_detached() { # dir prompt
    claude_launch_detached "$1" "$2"
}

agent_claude_resume() { # dir id [effort]
    local effort_flags=()
    if [[ -n ${3:-} ]]; then
        require_effort "$3"
        effort_flags=(--effort "$3")
    fi
    ensure_manager
    loading_overlay "Resuming $(basename "$1")${3:+ · effort $3} …" "$1" "$2"
    tmux respawn-pane -k -t "$(viewer_pane)" -c "$1" \
        env -u TMUX -u TMUX_PANE CLAUDE_SESSIONS_INSIDE=1 CLAUDE_SESSIONS_CREDITS="$launch_credits" \
        fish -ic 'echo; echo "  ⟳ Resuming the conversation in $PWD …"; echo; claude --resume $argv'"$CLAUDE_LAUNCH_EPILOGUE" -- "$2" "${effort_flags[@]}"
    : > "$CURRENT"
    label_panes "$(basename "$1")"
    focus_viewer
}

# A restart keeps the session's effort unless a new one is given: the level the status line last
# reported (statusline-command.sh copies CLAUDE_EFFORT, which Claude exports to its children).
# Its billing too, unless one is given (billing): API credits when the session reports it.
agent_claude_restart() { # id dir [effort] [detached] [credits|subscription]
    local id=$1 directory=$2 effort=${3:-} detached=${4:-} billing=${5:-} asked_billing=${5:-}
    if [[ -z $billing ]]; then
        billing=subscription
        [[ $(session_field "$id" credits) == true ]] && billing=credits
    fi
    launch_credits=0
    [[ $billing == credits ]] && launch_credits=1
    if [[ -n $effort ]]; then
        require_effort "$effort" || return 1
    else
        effort=$(jq -r '.effort // empty' "$HOME/.cache/claude/statusline-$id.json" 2>/dev/null || true)
        require_effort "$effort" 2>/dev/null || effort=""
    fi
    if [[ $detached == detached ]]; then
        exit_session "$id" || return 1
        if has_conversation "$id"; then
            claude_launch_detached "$directory" --resume "$id" ${effort:+--effort "$effort"}
        else
            claude_launch_detached "$directory" ${effort:+--effort "$effort"}
        fi
        return
    fi
    # Launched in a tmux session of its own, as restart-all does: in the viewer pane it died when the
    # column showed the next card as this one dropped out, and the loading screen held the cockpit.
    local since
    since=$(date +%s)
    loading_overlay "Restarting $(basename "$directory")${effort:+ · effort $effort}${asked_billing:+ · $billing} …" "$directory" "$id"
    exit_session "$id" || return 1
    if has_conversation "$id"; then
        claude_launch_detached "$directory" --resume "$id" ${effort:+--effort "$effort"}
        view_when_back "$id" "$directory" "$since"
    else
        # Nothing said yet: a fresh session in the same directory is the same thing, with a new id.
        loading_overlay "Restarting $(basename "$directory") …" "$directory"
        claude_launch_detached "$directory" ${effort:+--effort "$effort"}
        view_when_back "" "$directory" "$since"
    fi
}

# A conversation is resumed; a session nobody typed in yet has none, and starts fresh in its folder.
agent_claude_restore() { # dir id
    local directory=$1 id=$2 effort
    effort=$(jq -r '.effort // empty' "$HOME/.cache/claude/statusline-$id.json" 2>/dev/null || true)
    require_effort "$effort" 2>/dev/null || effort=""
    launch_credits=0
    [[ $(jq -r --arg id "$id" '.[$id].credits // false' "$RECENT_SESSIONS" 2>/dev/null) == true ]] && launch_credits=1
    if has_conversation "$id"; then
        claude_launch_detached "$directory" --resume "$id" ${effort:+--effort "$effort"}
    else
        log_event INFO manager "restore: $id had no conversation yet, a fresh session starts in $directory"
        claude_launch_detached "$directory" ${effort:+--effort "$effort"}
    fi
}

# At the idle prompt /exit is the clean way out. Otherwise two Ctrl-C in quick succession: the
# first interrupts the turn or clears typed input, the second must follow within Claude's short
# "press again to exit" window. Two rounds with a one-second grace each, then force-close.
agent_claude_exit() { # tmux-session pane id
    local target=$1 pane=$2 id=$3 attempt
    for attempt in 1 2; do
        if pane_at_prompt "$pane"; then
            submit_to_pane "$pane" "/exit"
        else
            tmux send-keys -t "$pane" C-c
            sleep 0.3
            tmux send-keys -t "$pane" C-c
        fi
        if claude_left "$target" "$pane" 1; then
            # What claude-auto-retry left behind, if anything.
            if tmux has-session -t "=$target" 2>/dev/null; then tmux kill-session -t "=$target" 2>/dev/null || true; fi
            return 0
        fi
    done
    echo "claude-sessions: $id did not exit, killing its tmux session" >&2
    tmux kill-session -t "=$target"
}

# Sent at its prompt, queued with Claude's own Ctrl+X Enter while it works, refused while a dialog
# is open (it would answer it).
agent_claude_say() { # id pane message
    local id=$1 pane=$2 message=$3 tail
    tail=$(tmux capture-pane -p -t "$pane" -S -20 2>/dev/null)
    if [[ $(osd classify <<<"$tail") == needs-you ]]; then
        echo "claude-sessions: $(session_field "$id" project) has a dialog open; answer it first (Y or N)" >&2
        return 1
    fi
    tmux send-keys -t "$pane" -l -- "$message"
    if pane_at_prompt "$pane"; then
        tmux send-keys -t "$pane" Enter
    else
        tmux send-keys -t "$pane" C-x Enter
    fi
    audit_event manager say sent "session=$id" "project=$(session_field "$id" project)" "fp:message=$message"
}

# What is left of the cursor when it sits at the start of Claude's prompt: spaces or the box's │,
# then ❯ and the space or no-break space Claude draws after it.
agent_claude_prompt_start() {
    printf '%s\n' $'^[[:space:]\u2502]*\u276f[ \u00a0]$'
}
