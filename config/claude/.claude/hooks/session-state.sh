#!/usr/bin/env bash
# Claude Code hooks, and opencode's cockpit plugin (~/.config/opencode/plugins/cockpit.ts, which
# sends the same payloads with COCKPIT_AGENT=opencode) → the OSD daemon (osd, apps/desktop): one `session <json>` line per event with
# the session's state (working | waiting | needs-you | gone), its project, the focus target of its
# window (KITTY_* can be stale after a tmux re-attach; the daemon re-resolves it) and an optional
# banner message. The daemon keeps the registry, colours the Hyprland window
# border, feeds the waybar chip and shows the top-centre banners. A swaync toast is only the fallback
# when the OSD cannot be reached (daemon down, or a sandbox without the FIFO).
# Runs inside greywall: the session bus is exposed (notifications only) and $XDG_RUNTIME_DIR/claude-report
# is in allowRead (connecting needs no more; writable, a session could replace the socket).
set -euo pipefail

# A nested claude (a `claude -p` that a tool of another session runs) is not a session of yours:
# it shares the parent's pane and would shadow it in the manager, so its hooks report nothing.
# Claude Code marks every process a tool runs with CLAUDECODE=1, opencode with OPENCODE=1; a
# session started by you from a shell has no such mark, a nested one carries it in its own
# environment (opencode sets its own mark after it started, so /proc does not show it on itself).
# Counting processes named claude does not work since 2.1.300: the launcher, the daemon and the
# pty host are all ancestors of every session. The marked ancestor is looked for among agent
# processes only (comm claude, a version number or opencode), so the hook's own tool shells never
# count.
is_agent_process() { # pid
    local comm
    comm=$(cat "/proc/$1/comm" 2>/dev/null) || return 1
    [[ $comm == claude || $comm == opencode || $comm =~ ^[0-9]+\.[0-9]+\.[0-9]+ ]]
}
nested=0
pid=$$
while (( pid > 1 )); do
    pid=$(awk '/^PPid:/ { print $2 }' "/proc/$pid/status" 2>/dev/null) || break
    [[ -n $pid ]] || break
    if is_agent_process "$pid" && tr '\0' '\n' < "/proc/$pid/environ" 2>/dev/null | grep -qxE 'CLAUDECODE=1|OPENCODE=1'; then
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
if [[ ! $session =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ && ! $session =~ ^ses_[0-9A-Za-z]{12,40}$ ]]; then
    echo "session-state.sh: missing or invalid session id" >&2
    exit 2
fi
input_file="$HOME/.cache/claude/input-$session.question"
pending_input_file="$input_file.pending"
# A socket, not the FIFO: the daemon learns which process sent the report and takes the tmux pane
# from where it really runs (a sandbox could otherwise claim another session's pane).
report_socket="${XDG_RUNTIME_DIR:-/tmp}/claude-report/osd-report.sock"
toast_id_file="$HOME/.cache/claude/toast-$session.id"

# What a turn left in the repository, worked out here, inside the session's sandbox: git reads the
# repository's own configuration, which the session can write, so it must never run outside.
turn_head_file="$HOME/.cache/claude/turn-$session.head"
turn_id_file="$HOME/.cache/claude/turn-$session.id"
turn_done_file="$HOME/.cache/claude/turn-$session.done"
start_turn() {
    mkdir -p "$(dirname "$turn_id_file")"
    od -An -N16 -tx1 /dev/urandom | tr -d ' \n' > "$turn_id_file"
    rm -f "$turn_done_file"
}
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

# Background work still in flight when a turn ends (the Stop payload's background_tasks), for the
# card's "◷ 3 agents": agents (subagents, teammates, cloud sessions) apart from the rest (shells,
# monitors, workflows, MCP tasks). Claude's own housekeeping (dream, auto-mode scan, memory import)
# is not the session's work. Prints nothing when the payload has no list (an older Claude).
AGENT_TASKS='["subagent", "teammate", "cloud session"]'
HOUSEKEEPING_TASKS='["dream", "auto-mode scan", "memory import"]'
background_of() { # payload: "<agents> <other work>"
    jq -r --argjson agents "$AGENT_TASKS" --argjson housekeeping "$HOUSEKEEPING_TASKS" '
        .background_tasks // empty
        | [(map(select(.type | IN($agents[]))) | length),
           (map(select(.type | IN(($agents + $housekeeping)[]) | not)) | length)]
        | @tsv' <<<"$1"
}

state=
task=
label=
message=
outcome=
turn_id=
turn_complete=false
repository=
branch=
repository_reported=false
unpushed=
agents=
background=
sticky=false
# Started with "this terminal" (the claude function's t): the cockpit keeps it out of what acts on
# every session.
terminal=false
[[ ${COCKPIT_TERMINAL:-} == 1 ]] && terminal=true
# Billed to the Console's API credits (the cockpit's $): always reported, false included, so the
# card follows a switch back to the subscription.
credits=false
[[ ${COCKPIT_CREDITS:-} == 1 ]] && credits=true
# opencode's plugin says so; Claude's hooks say nothing, which the daemon takes for claude.
agent=${COCKPIT_AGENT:-}
needs_input=
case $event in
    TaskLabel)
        if ! label=$(jq -er '.label | select(type == "string")
            | select((test("[\\x00-\\x1f\\x7f-\\x9f]") | not) and (contains("#(") | not) and (contains("#{") | not))
            | gsub("\\s+"; " ") | sub("^ +"; "") | sub(" +$"; "")
            | select(length <= 60 and (split(" ") | length >= 3 and length <= 6))' <<<"$payload"); then
            echo "session-state.sh: task label must be 3–6 words, at most 60 characters, without controls or tmux formats" >&2
            exit 2
        fi
        ;;
    NeedsInput)
        if ! jq -e '.message | type == "string" and (length > 0 and length <= 300) and test("\\S") and
            (test("[\\x00-\\x1f\\x7f]") | not) and (contains("#(") | not) and (contains("#{") | not)' <<<"$payload" >/dev/null; then
            echo "session-state.sh: question must be 1–300 characters, not blank, without controls or tmux formats" >&2
            exit 2
        fi
        mkdir -p "$(dirname "$input_file")"
        jq -r '.message' <<<"$payload" > "$pending_input_file"
        rm -f "$input_file"
        state=working
        ;;
    SessionStart)
        rm -f "$turn_id_file" "$turn_done_file"
        rm -f "$input_file" "$pending_input_file"
        needs_input=false
        state=waiting
        unpushed=$(unpushed_of "$cwd")
        # Background work dies with the process: a new or resumed one has none. /clear and
        # /compact keep the process, and its work.
        case $(jq -r '.source // ""' <<<"$payload") in
            startup | resume) agents=0 background=0 ;;
        esac
        ;;
    UserPromptSubmit)
        start_turn
        state=working
        # The commit the turn starts from, to tell what it did when it stops.
        { mkdir -p "$(dirname "$turn_head_file")"; git -C "$cwd" rev-parse HEAD > "$turn_head_file"; } 2>/dev/null || rm -f "$turn_head_file"
        # Only what you typed names the task: prompts from the system (task notifications, loop
        # wake-ups) say nothing about it.
        if [[ $(jq -r '.source // "user"' <<<"$payload") == user ]]; then
            rm -f "$input_file" "$pending_input_file"
            needs_input=false
            task=$(jq -r '.prompt // "" | split("\n")[0] | gsub("\\s+"; " ") | .[:60]' <<<"$payload")
        fi
        ;;
    PostToolUse)
        if [[ -f $turn_done_file ]]; then start_turn; fi
        state=working
        ;;
    Notification)
        message=$(jq -r '.message | gsub("\\s+"; " ")' <<<"$payload")
        # Requests for a human stick and turn the session red; turn-end notices leave it waiting;
        # anything informational (auth, quota, computer use, plugins) shows its message and keeps the state.
        case $(jq -r '.notification_type // ""' <<<"$payload") in
            permission_prompt | worker_permission_prompt | elicitation_dialog | elicitation_url_dialog | agent_needs_input)
                state=needs-you; sticky=true; needs_input=false ;;
            idle_prompt) state=waiting; sticky=true ;;
            agent_completed | push_notification) state=waiting ;;
            *) state= ;;
        esac
        ;;
    Stop)
        turn_id=$(cat "$turn_id_file" 2>/dev/null || true)
        if [[ ! $turn_id =~ ^[0-9a-f]{32}$ ]]; then
            start_turn
            turn_id=$(<"$turn_id_file")
        fi
        touch "$turn_done_file"
        turn_complete=true
        # The command runs before the final reply: only turn-end makes its question answerable.
        if [[ -f $pending_input_file ]]; then mv -f "$pending_input_file" "$input_file"; fi
        state=waiting
        message="Finished, waiting for you"
        outcome=$(outcome_of_turn "$cwd")
        unpushed=$(unpushed_of "$cwd")
        read -r agents background < <(background_of "$payload") || true
        checks=$(checks_of_turn "$(jq -r '.transcript_path // ""' <<<"$payload")")
        if [[ -n $checks ]]; then outcome="${outcome:+$outcome · }$checks"; fi
        ;;
    SessionEnd)
        rm -f "$input_file" "$pending_input_file"
        needs_input=false
        state=gone
        rm -f "$turn_head_file" "$turn_id_file" "$turn_done_file"
        ;;
    *)
        echo "session-state.sh: unexpected hook event $event" >&2
        exit 1
        ;;
esac

if [[ $event == SessionStart || $event == Stop ]]; then
    repository_reported=true
    origin=$(git -C "$cwd" remote get-url origin 2>/dev/null || true)
    origin=${origin%.git}
    if [[ $origin =~ ^(https://github\.com/|git@github\.com:|ssh://git@github\.com/)([A-Za-z0-9_.-]+)/([A-Za-z0-9_.-]+)$ ]] \
        && [[ ${BASH_REMATCH[2]} != . && ${BASH_REMATCH[2]} != .. && ${BASH_REMATCH[3]} != . && ${BASH_REMATCH[3]} != .. ]]; then
        repository="${BASH_REMATCH[2],,}/${BASH_REMATCH[3],,}"
        (( ${#repository} <= 200 )) || repository=
    fi
    branch=$(git -C "$cwd" symbolic-ref --quiet --short HEAD 2>/dev/null || true)
    # Git allows names the daemon refuses as text; one would drop the whole report, Stop included.
    if [[ $branch == *'#{'* || $branch == *'#('* ]] || (( ${#branch} > 300 )); then
        branch=
    fi
fi

case $event in
    NeedsInput | PostToolUse | Stop | Notification | UserPromptSubmit)
        [[ $event == Notification ]] || needs_input=false
        # A real dialog supersedes a saved chat question; restore it after the dialog/turn ends.
        if [[ -f $input_file && ! ( $event == Notification && $needs_input == false ) ]]; then
            message=$(<"$input_file")
            state=needs-you
            sticky=true
            needs_input=true
        fi
        ;;
esac

report=$(jq -cn \
    --arg session "$session" --arg state "$state" --arg project "$project" --arg cwd "$cwd" \
    --arg task "$task" --arg label "$label" --arg message "$message" --arg outcome "$outcome" --argjson sticky "$sticky" --argjson terminal "$terminal" --argjson credits "$credits" --arg unpushed "$unpushed" --arg agents "$agents" --arg background "$background" --arg apps "$(craft_apps)" --arg agent "$agent" \
    --arg needs_input "$needs_input" --arg kitty_pid "${KITTY_PID:-}" --arg kitty_window "${KITTY_WINDOW_ID:-}" \
    --arg turn_id "$turn_id" --argjson turn_complete "$turn_complete" \
    --arg repository "$repository" --arg branch "$branch" --argjson repository_reported "$repository_reported" \
    --arg kitty_listen "${KITTY_LISTEN_ON:-}" --arg tmux_pane "${TMUX_PANE:-}" \
    '{$session, $state, $project, $cwd, $task, $label, $message, $outcome, $sticky, $terminal,
      $kitty_pid, $kitty_window, $kitty_listen, $tmux_pane}
     | with_entries(select(.value != "" and .value != false))
     | if $unpushed == "" then . else .unpushed = ($unpushed | tonumber) end
     | if $agents == "" then . else .agents = ($agents | tonumber) | .background = ($background | tonumber) end
     | if $agent == "" then . else .agent = $agent end
      | if $needs_input == "" then . else .needs_input = ($needs_input == "true") end
      | if $turn_complete then .turn_id = $turn_id | .turn_complete = true else . end
      | if $repository_reported then .repository = $repository | .branch = $branch else . end
     | .apps = $apps | .credits = $credits')

# One line per event in ~/.cache/claude/hook.log, the one place the sandbox lets the hook write:
# the event, the session, the state and where the report went, never prompt text. 512 KB, then
# one previous file. `cockpit logs` shows the end of it.
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
    local request=$1 reply
    case $event in NeedsInput | TaskLabel) request="session-request $report" ;; esac
    if ! reply=$(printf '%s\n' "$request" | timeout 1 socat -t1 - "UNIX-CONNECT:$report_socket" 2>/dev/null); then
        echo "session-state.sh: osd report socket not answering, is osd running?" >&2
        return 1
    fi
    case $event in
        NeedsInput | TaskLabel)
            if [[ $reply != ok ]]; then
                echo "session-state.sh: osd did not accept $event: ${reply:-no acknowledgement}" >&2
                return 1
            fi
            ;;
    esac
}

if send_to_osd "session $report"; then
    hook_log "sent to the osd daemon"
    exit 0
fi
hook_log "osd daemon not reading $report_socket"
if [[ $event == NeedsInput ]]; then
    echo "session-state.sh: cannot flag the question: osd is not reading $report_socket" >&2
    exit 1
fi
if [[ $event == TaskLabel ]]; then
    echo "session-state.sh: cannot publish the task label: osd is not reading $report_socket" >&2
    exit 1
fi
hook_log "falling back to a toast"

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
