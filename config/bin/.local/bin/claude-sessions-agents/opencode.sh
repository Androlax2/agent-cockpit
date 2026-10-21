# opencode, as claude-sessions runs it (sourced by claude-sessions and claude-sessions-left).
# The verbs every agent defines are listed in claude-sessions (AGENT_VERBS); README.md in this
# folder says what each one does. Its cockpit plugin (~/.config/opencode/plugins/cockpit.ts)
# reports its states like a Claude session's hooks.
# shellcheck shell=bash

agent_opencode_label() { echo "opencode"; }

# ses_, then 12 to 40 letters or digits (packages/schema/src/identifier.ts).
agent_opencode_is_session_id() { # id
    [[ $1 =~ ^ses_[0-9A-Za-z]{12,40}$ ]]
}

# Before its first report an opencode session is its tmux session.
OPENCODE_TMUX_PREFIX=claude-retry-opencode-
agent_opencode_tmux_prefix() { echo "$OPENCODE_TMUX_PREFIX"; }

# Effort restarts, billing, /compact and Remote Control are Claude's. OpenCode changes effort
# through its native variant picker and redraws itself on a resize (no Ctrl+L redraw).
agent_opencode_can() { # feature
    [[ $1 == variants ]]
}

# opencode in a tmux session of its own, named like claude-auto-retry's (which is Claude's alone)
# so the manager views, exits and lists it like any other. A failed start is logged and stays on
# screen until a key. Prints the tmux session's name.
# --standalone: a server of its own, which ends with it (opencode 2 otherwise shares a background
# server that outlives the session, and its cockpit plugin with it). --session: the id the card
# carries from the start; opencode makes the session with it at the first prompt. ask-to-run and
# claude-todo read it from CLAUDE_CODE_SESSION_ID, as in a Claude session.
OPENCODE_LAUNCHER="${CLAUDE_SESSIONS_OPENCODE:-$HOME/.local/bin/greywall-agents/opencode}"
# OpenCode can return 255 on the manager's Ctrl-C close. Only that requested exit is benign;
# a 255 without a request (or any other failure) must still reach the error hand-off.
OPENCODE_EPILOGUE='"$@" && exit; code=$?; if [ "$code" = 255 ] && [ "$(tmux show-options -pqv -t "$TMUX_PANE" @opencode_exit_requested 2>/dev/null)" = 1 ]; then exit 0; fi; claude-sessions log manager ERROR "opencode exited with status $code in $PWD"; printf "\n  ✗ opencode exited with status %s\n  any key closes this pane " "$code"; read -r _'

start_opencode() { # dir session-id model-ref opencode-arguments...
    local directory=$1 session=$2 model=$3 name size
    local -a terminal_environment=()
    shift 3
    require_plain_directory "$directory"
    name="$OPENCODE_TMUX_PREFIX$(date +%s%N)"
    size=$(detached_size)
    if [[ -n ${CLAUDE_SESSIONS_TERMINAL:-} ]]; then
        terminal_environment=(CLAUDE_SESSIONS_TERMINAL="$CLAUDE_SESSIONS_TERMINAL")
    fi
    if [[ -n $model ]]; then terminal_environment+=(CLAUDE_SESSIONS_OPENCODE_MODEL="$model"); fi
    if [[ -n ${launch_team_spawn:-} ]]; then terminal_environment+=(CLAUDE_SESSIONS_TEAM_SPAWN="$launch_team_spawn"); fi
    tmux new-session -d -s "$name" -x "${size% *}" -y "${size#* }" -c "$directory" -e "CLAUDE_CODE_SESSION_ID=$session" \
        env -u CLAUDE_SESSIONS_OPENCODE_MODEL CLAUDE_SESSIONS_INSIDE=1 CLAUDE_CODE_SESSION_ID="$session" "${terminal_environment[@]}" \
        sh -c "$OPENCODE_EPILOGUE" opencode "$OPENCODE_LAUNCHER" --standalone --session "$session" "$@" || return
    printf '%s\n' "$name"
}

opencode_default_model() {
    local model effort
    model=$(launch_setting_value opencode-model) || return
    effort=$(launch_setting_value opencode-effort) || return
    validate_launch_setting opencode-model "$model" || return
    validate_launch_setting opencode-effort "$effort" || return
    if [[ -z $model && -n $effort ]]; then
        echo "claude-sessions: pick an opencode-model before its effort" >&2; return 1
    fi
    printf '%s' "$model"
    [[ -z $effort ]] || printf '#%s' "$effort"
    return 0
}

start_new_opencode() { # dir opencode-arguments...
    local directory=$1 model
    shift
    model=$(opencode_default_model) || return
    start_opencode "$directory" "$(new_opencode_session_id)" "$model" "$@"
}

# Use tmux's own pane inventory, as the cockpit does before the first plugin report, not a
# reported id's claimed pane. Never change a reused session's terminal/automation ownership.
agent_opencode_terminal() ( # dir
    local directory=$1 name pane folder dead target="" choice index lock_key
    local -a matches=()
    directory=$(CDPATH= cd -- "$directory" && pwd -P) || return
    require_plain_directory "$directory"
    if [[ -n ${CLAUDE_SESSIONS_INSIDE:-} ]]; then
        echo "claude-sessions: already inside a managed session" >&2
        return 1
    fi
    # Two terminals arriving before the first plugin report must still create only one process.
    lock_key=$(printf '%s' "$directory" | sha256sum)
    exec 9>"${XDG_RUNTIME_DIR:-/tmp}/claude-sessions-opencode-${lock_key%% *}.lock"
    flock 9
    while IFS=$'\t' read -r name pane folder dead; do
        [[ $name =~ ^claude-retry-opencode-[0-9]+$ && $pane =~ ^%[0-9]+$ && $folder == "$directory" && $dead == 0 ]] || continue
        [[ " ${matches[*]} " == *" $name "* ]] || matches+=("$name")
    done < <(tmux list-panes -a -F '#{session_name}	#{pane_id}	#{pane_current_path}	#{pane_dead}' 9>&-)
    case ${#matches[@]} in
        0) target=$(CLAUDE_SESSIONS_TERMINAL=1 start_new_opencode "$directory" 9>&-) || return ;;
        1) target=${matches[0]} ;;
        *)
            for index in "${!matches[@]}"; do printf '%s) %s\n' "$((index + 1))" "${matches[index]}" >&2; done
            printf 'OpenCode session (number, Enter cancels): ' >&2
            IFS= read -r choice || return 130
            [[ $choice =~ ^[1-9][0-9]{0,5}$ ]] || return 1
            (( choice <= ${#matches[@]} )) || return 1
            target=${matches[choice-1]}
            ;;
    esac
    flock -u 9
    exec 9>&-
    if [[ -n ${TMUX:-} ]]; then
        tmux switch-client -t "=$target"
    else
        tmux attach-session -t "=$target"
    fi
)

# A new session id the way opencode makes them (packages/schema/src/identifier.ts): ses_, the time
# counted down in 12 hex digits (newest first in its lists), then 14 random letters and digits.
new_opencode_session_id() {
    local countdown=$(( ~($(date +%s%3N) * 4096 + 1) & 0xFFFFFFFFFFFF )) letters
    letters=$(LC_ALL=C tr -dc 'A-Za-z0-9' < /dev/urandom | head -c 14)
    printf 'ses_%012x%s\n' "$countdown" "$letters"
}

# No loading screen: opencode reports at its first prompt only (it makes its session then), and
# its own screen is up in a second.
agent_opencode_new() { # dir [opencode arguments]
    local directory=$1 target
    shift
    target=$(start_new_opencode "$directory" "$@") || return
    view_session "$target"
    label_panes "$(basename "$directory")"
}

# opencode takes its first message as --prompt: a bare argument would be its project folder.
agent_opencode_new_with_prompt() { # dir prompt
    agent_opencode_new "$1" --prompt "$2"
}

agent_opencode_background() { # dir prompt
    start_new_opencode "$1" --prompt "$2" >/dev/null
}

agent_opencode_detached() { # dir prompt
    start_new_opencode "$1" --prompt "$2" >/dev/null
}

agent_opencode_resume() { # dir id
    view_session "$(start_opencode "$1" "$2" "")"
    label_panes "$(basename "$1")"
    focus_viewer
}

# opencode makes its session at the first prompt, so a session it reported has a conversation.
agent_opencode_restart() { # id dir [effort] [detached]
    local id=$1 directory=$2 detached=${4:-}
    exit_session "$id" || return 1
    if [[ $detached == detached ]]; then
        start_opencode "$directory" "$id" "" >/dev/null
        return
    fi
    view_session "$(start_opencode "$directory" "$id" "")"
    label_panes "$(basename "$directory")"
}

agent_opencode_restore() { # dir id
    start_opencode "$1" "$2" "" >/dev/null
}

# Escape twice interrupts a turn, Ctrl-C clears typed input, Ctrl-C at an empty prompt exits. Two
# rounds with a one-second grace each, then force-close.
agent_opencode_exit() { # tmux-session pane id
    local attempt error
    # The pane can close between any two calls; a failed call is only success if its session is gone.
    error=$(tmux set-option -p -t "$2" @opencode_exit_requested 1 2>&1) || { opencode_exit_call_failed "$1" "$error"; return; }
    for attempt in 1 2; do
        error=$(tmux send-keys -t "$2" Escape Escape 2>&1) || { opencode_exit_call_failed "$1" "$error"; return; }
        sleep 0.2
        error=$(tmux send-keys -t "$2" C-c 2>&1) || { opencode_exit_call_failed "$1" "$error"; return; }
        sleep 0.3
        error=$(tmux send-keys -t "$2" C-c 2>&1) || { opencode_exit_call_failed "$1" "$error"; return; }
        if claude_left "$1" "$2" 1; then
            # A failed exit leaves the start's epilogue waiting for a key.
            if tmux has-session -t "=$1" 2>/dev/null; then tmux kill-session -t "=$1" 2>/dev/null || true; fi
            return 0
        fi
    done
    echo "claude-sessions: opencode in $1 did not exit, killing its tmux session" >&2
    tmux kill-session -t "=$1"
}

# A background exit's stderr is logged as an ERROR, so tmux's error is shown only while the session
# still runs: once it is gone, the exit succeeded and "can't find pane" is just the race.
opencode_exit_call_failed() { # tmux-session error
    tmux has-session -t "=$1" 2>/dev/null || return 0
    printf '%s\n' "$2" >&2
    return 1
}

# Its screen is not Claude's: the daemon's state, from its plugin, says what it does. At its
# prompt the message is sent; while it works, opencode's inbox queues it for after the turn (as
# Claude's Ctrl+X Enter). Only an explicit chat question accepts text in needs-you;
# real permission and form dialogs must never receive a chat message.
agent_opencode_say() { # id pane message
    local id=$1 pane=$2 message=$3 state
    state=$(session_field "$id" state)
    if [[ $state == needs-you && $(session_field "$id" needs_input) != true ]]; then
        echo "claude-sessions: $(session_field "$id" project) has a question open; answer it in its pane first" >&2
        return 1
    fi
    if [[ $state != waiting && $state != working && $state != needs-you ]]; then
        echo "claude-sessions: $(session_field "$id" project) is not at its prompt (${state:-no state yet})" >&2
        return 1
    fi
    submit_to_pane "$pane" "$message"
    audit_event manager say sent "session=$id" "project=$(session_field "$id" project)" "fp:message=$message"
}

# What is left of the cursor at the start of its input box: spaces, its heavy bar ┃ and the two
# spaces before the text.
agent_opencode_prompt_start() {
    printf '%s\n' $'^[[:space:]]*\u2503  $'
}

# The doctor's opencode section (an optional verb); report and failures are the doctor's. The
# key's shape is checked (its prefix and length), never shown.
agent_opencode_doctor() {
    local binary=${OPENCODE_BINARY:-$HOME/.opencode/bin/opencode} version key key_kind newest_log settings own_folders folder hidden=""
    local -a folders
    echo "opencode"
    if [[ ! -x $binary ]]; then
        report fail "opencode not installed at $binary (the cockpit offers it as an agent)" "curl -fsSL https://opencode.ai/v2/install | bash"
        return
    fi
    version=$("$binary" --version 2>/dev/null || true)
    report ok "installed: ${version:-version unknown}"
    # The newest log that loaded any plugin says whether the cockpit's was among them.
    newest_log=$(grep -l 'msg="loading plugin"' "${XDG_DATA_HOME:-$HOME/.local/share}"/opencode/log/*.log 2>/dev/null | xargs -r ls -t 2>/dev/null | head -1 || true)
    if [[ -z $newest_log ]]; then
        report ok "plugin: no opencode run logged yet"
    elif grep -q 'msg="loading plugin" id=[^ ]*/plugins/cockpit\.ts' "$newest_log"; then
        report ok "the cockpit plugin loaded in its last run ($(basename "$newest_log"))"
    else
        report fail "the cockpit plugin did not load in opencode's last run ($newest_log)" "ls -l ~/.config/opencode/plugins (cd ~/dotfiles/config && stow -t ~ --restow opencode)"
    fi
    key=$(secret-tool lookup service agent-opencode-anthropic 2>/dev/null || true)
    if [[ -z $key ]]; then
        report fail "no Anthropic API key for opencode in the keyring" "make -C ~/dotfiles agent-opencode-key"
    elif [[ $key =~ ^(sk-ant-[a-z]+[0-9]*)-[A-Za-z0-9_-]+$ ]] && (( ${#key} >= 80 )); then
        key_kind=${BASH_REMATCH[1]}
        report ok "API key in the keyring: $key_kind-… (${#key} characters)"
    else
        report fail "the keyring's opencode key does not look like an Anthropic API key (${#key} characters)" "make -C ~/dotfiles agent-opencode-key"
    fi
    unset key
    # Its own folders, from its launcher, must have left denyRead in its newest settings file.
    settings=$(ls -t "${XDG_RUNTIME_DIR:-/tmp}"/greywall-agents/*.opencode.json 2>/dev/null | head -1 || true)
    local launcher
    launcher="$(dirname "$AGENTS_DIR")/greywall-agents/opencode"
    own_folders=$(sed -n 's/^own_folders="\(.*\)"$/\1/p' "$launcher" 2>/dev/null)
    if [[ -z $settings ]]; then
        report ok "sandbox: no opencode session started since boot"
    elif [[ -z $own_folders ]]; then
        report fail "no own_folders in $launcher" "cd ~/dotfiles/config && stow -t ~ --restow bin"
    else
        read -ra folders <<< "$own_folders" # no glob: the entries are greywall's patterns, ** included
        for folder in "${folders[@]}"; do
            jq -e --arg folder "$folder" '.filesystem.denyRead | index($folder)' "$settings" >/dev/null 2>&1 && hidden+=" $folder"
        done
        if [[ -z $hidden ]]; then report ok "its sandbox sees its own data ($(basename "$settings"))"; else report fail "its sandbox hides its own data:$hidden ($settings)" "restart the opencode session (r): its launcher writes the settings again"; fi
    fi
}
