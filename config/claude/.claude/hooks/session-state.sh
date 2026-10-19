#!/usr/bin/env bash
# Claude Code hooks → the OSD daemon (osd, apps/desktop): one `session <json>` line per event with
# the session's state (working | waiting | needs-you | gone), its project, the focus target of its
# window (KITTY_* can be stale after a tmux re-attach; the daemon re-resolves it) and an optional
# banner message. The daemon keeps the registry, colours the Hyprland window
# border, feeds the waybar chip and shows the top-centre banners. A swaync toast is only the fallback
# when the OSD cannot be reached (daemon down, or a sandbox without the FIFO).
# Runs inside greywall: the session bus is exposed (notifications only) and $XDG_RUNTIME_DIR/osd-report.sock
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
# A socket, not the FIFO: the daemon learns which process sent the report and takes the tmux pane
# from where it really runs (a sandbox could otherwise claim another session's pane).
report_socket="${XDG_RUNTIME_DIR:-/tmp}/osd-report.sock"
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

# What the turn's own transcript says: the result of its last test run, and whether it closed a
# todo item (claude-todo done). Read here, inside the sandbox, like the git outcome above; only the
# end of the transcript, so a long session does not slow every stop down.
# A command counts where a command starts (line start, after ; & | and or, or an opening quote as in
# ask-to-run's), not inside some text that mentions it.
COMMAND_START='(^|[;&|]|\band|\bor|\s'"'"')\s*'
TEST_COMMAND="${COMMAND_START}(make (test|check)|cargo (test|nextest)|(npm|pnpm|yarn|bun)( run)? test|pytest|(vendor/bin/)?phpunit|composer test|php artisan test|go test|vitest|jest)\\b"
checks_of_turn() { # transcript
    [[ -r $1 ]] || return 0
    tail -n 3000 "$1" | jq -Rnr --arg tests "$TEST_COMMAND" --arg todo "${COMMAND_START}claude-todo done\\b" '
        [inputs | fromjson? | select(type == "object")] as $entries
        | ($entries | to_entries
           | map(select(.value.type == "user" and ((.value.message.content | type) == "string"
                         or ([.value.message.content[]? | .type?] | index("tool_result") | not))))
           | last | .key // 0) as $start
        | $entries[$start:] as $turn
        | ([$turn[] | select(.type == "assistant") | .message.content[]? | select(.type? == "tool_use" and .name == "Bash")
            | {id, command: (.input.command // "")}]) as $commands
        | ([$turn[] | select(.type == "user") | .message.content[]? | select(type == "object" and .type == "tool_result")
            | {id: .tool_use_id, failed: (.is_error // false)}]) as $results
        | def ran($pattern): [$results[] | . as $result | select($commands | any(.id == $result.id and (.command | test($pattern))))];
          (ran($tests) | last) as $last_tests
        | (ran($todo) | map(select(.failed | not)) | length) as $todos
        | [ (if $last_tests == null then empty elif $last_tests.failed then "tests failed" else "tests passed" end),
            (if $todos > 0 then "todo done" else empty end) ]
        | join(" · ")' 2>/dev/null || true
}

# Commits of the repository on no remote yet, for the card's "↑N unpushed". Only in a repository
# with a remote; git runs here, inside the sandbox, like the outcome above.
unpushed_of() { # cwd
    git -C "$1" remote 2>/dev/null | grep -q . || return 0
    local live="$HOME/dotfiles"
    if [[ $(git -C "$1" remote get-url origin 2>/dev/null) == "$live" ]]; then
        unapplied_of "$1" "$live"
        return
    fi
    git -C "$1" rev-list --count HEAD --not --remotes 2>/dev/null || true
}

# A clone of the live dotfiles (its origin) brings its commits over with apply-dotfiles, never a
# push, and its origin/main is only as fresh as its last fetch: what counts is the commits whose
# change live main lacks. git am gives an applied commit a new hash, so changes are compared by
# patch id. Read-only: the launcher mounts the live checkout read-only for these clones.
unapplied_of() { # cwd live
    local base
    base=$(git -C "$1" merge-base HEAD refs/remotes/origin/main 2>/dev/null) || return 0
    patch_ids() { git -C "$1" log -p --no-color --no-merges "$2" 2>/dev/null | git patch-id --stable | cut -d' ' -f1; }
    patch_ids "$1" "$base..HEAD" | grep -cvxF -f <(patch_ids "$2" "$base..main"; echo none) || true
}

# The desktop apps this session drives through an MCP server attached to them (`figcraft-cli mcp
# --attach PORT`, same for the other *craft apps), each "on" when its port answers: "figcraft:on".
# ps sees this sandbox's processes only, so these are this session's servers.
craft_apps() {
    local line app port apps=""
    while read -r line; do
        app=${line%%-cli *}
        app=${app##*/}
        port=$(grep -oE -- '--attach [0-9]+' <<<"$line" | grep -oE '[0-9]+$')
        [[ $app =~ ^[a-z]+craft$ && -n $port ]] || continue
        if (exec 3<>"/dev/tcp/127.0.0.1/$port") 2>/dev/null; then apps+="$app:on "; else apps+="$app:off "; fi
    done < <(ps -eo args= 2>/dev/null | grep -E '[a-z]+craft-cli mcp --attach [0-9]+' || true)
    echo "${apps% }"
}

state=
task=
message=
outcome=
unpushed=
sticky=false
# Started with "this terminal" (the claude function's t): the cockpit keeps it out of what acts on
# every session.
terminal=false
[[ ${CLAUDE_SESSIONS_TERMINAL:-} == 1 ]] && terminal=true
# Billed to the Console's API credits (the cockpit's $): always reported, false included, so the
# card follows a switch back to the subscription.
credits=false
[[ ${CLAUDE_SESSIONS_CREDITS:-} == 1 ]] && credits=true
case $event in
    SessionStart)
        state=waiting
        unpushed=$(unpushed_of "$cwd")
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
        unpushed=$(unpushed_of "$cwd")
        checks=$(checks_of_turn "$(jq -r '.transcript_path // ""' <<<"$payload")")
        if [[ -n $checks ]]; then outcome="${outcome:+$outcome · }$checks"; fi
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
    --arg task "$task" --arg message "$message" --arg outcome "$outcome" --argjson sticky "$sticky" --argjson terminal "$terminal" --argjson credits "$credits" --arg unpushed "$unpushed" --arg apps "$(craft_apps)" \
    --arg kitty_pid "${KITTY_PID:-}" --arg kitty_window "${KITTY_WINDOW_ID:-}" \
    --arg kitty_listen "${KITTY_LISTEN_ON:-}" --arg tmux_pane "${TMUX_PANE:-}" \
    '{$session, $state, $project, $cwd, $task, $message, $outcome, $sticky, $terminal,
      $kitty_pid, $kitty_window, $kitty_listen, $tmux_pane}
     | with_entries(select(.value != "" and .value != false))
     | if $unpushed == "" then . else .unpushed = ($unpushed | tonumber) end
     | .apps = $apps | .credits = $credits')

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

# A daemon that does not answer must not hold the hook: give up after a second.
send_to_osd() {
    [[ -S $report_socket ]] || return 1
    printf '%s\n' "$1" | timeout 1 socat -t1 - "UNIX-CONNECT:$report_socket" >/dev/null 2>&1 && return 0
    echo "session-state.sh: osd report socket not answering, is osd running?" >&2
    return 1
}

if send_to_osd "session $report"; then
    hook_log "sent to the osd daemon"
    exit 0
fi
hook_log "osd daemon not reading $report_socket: falling back to a toast"

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
