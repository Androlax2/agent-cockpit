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

# None of Claude Code's features: effort, billing, /compact and Remote Control are Claude's, and
# opencode redraws itself on a resize (no Ctrl+L redraw).
agent_opencode_can() { # feature
    return 1
}

# opencode in a tmux session of its own, named like claude-auto-retry's (which is Claude's alone)
# so the manager views, exits and lists it like any other. A failed start is logged and stays on
# screen until a key. Prints the tmux session's name.
# --standalone: a server of its own, which ends with it (opencode 2 otherwise shares a background
# server that outlives the session, and its cockpit plugin with it). --session: the id the card
# carries from the start; opencode makes the session with it at the first prompt. ask-to-run and
# claude-todo read it from CLAUDE_CODE_SESSION_ID, as in a Claude session.
OPENCODE_LAUNCHER="${CLAUDE_SESSIONS_OPENCODE:-$HOME/.local/bin/greywall-agents/opencode}"
OPENCODE_EPILOGUE='"$@" && exit; code=$?; claude-sessions log manager ERROR "opencode exited with status $code in $PWD"; printf "\n  ✗ opencode exited with status %s\n  any key closes this pane " "$code"; read -r _'

start_opencode() { # dir session-id opencode-arguments...
    local directory=$1 session=$2 name size
    shift 2
    require_plain_directory "$directory"
    name="$OPENCODE_TMUX_PREFIX$(date +%s%N)"
    size=$(detached_size)
    tmux new-session -d -s "$name" -x "${size% *}" -y "${size#* }" -c "$directory" \
        env CLAUDE_SESSIONS_INSIDE=1 CLAUDE_CODE_SESSION_ID="$session" \
        sh -c "$OPENCODE_EPILOGUE" opencode "$OPENCODE_LAUNCHER" --standalone --session "$session" "$@"
    printf '%s\n' "$name"
}

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
    local directory=$1
    shift
    view_session "$(start_opencode "$directory" "$(new_opencode_session_id)" "$@")"
    label_panes "$(basename "$directory")"
}

# opencode takes its first message as --prompt: a bare argument would be its project folder.
agent_opencode_new_with_prompt() { # dir prompt
    agent_opencode_new "$1" --prompt "$2"
}

agent_opencode_background() { # dir prompt
    start_opencode "$1" "$(new_opencode_session_id)" --prompt "$2" >/dev/null
}

agent_opencode_detached() { # dir prompt
    start_opencode "$1" "$(new_opencode_session_id)" --prompt "$2" >/dev/null
}

agent_opencode_resume() { # dir id
    view_session "$(start_opencode "$1" "$2")"
    label_panes "$(basename "$1")"
    focus_viewer
}

# opencode makes its session at the first prompt, so a session it reported has a conversation.
agent_opencode_restart() { # id dir [effort] [detached]
    local id=$1 directory=$2 detached=${4:-}
    exit_session "$id" || return 1
    if [[ $detached == detached ]]; then
        start_opencode "$directory" "$id" >/dev/null
        return
    fi
    view_session "$(start_opencode "$directory" "$id")"
    label_panes "$(basename "$directory")"
}

agent_opencode_restore() { # dir id
    start_opencode "$1" "$2" >/dev/null
}

# Escape twice interrupts a turn, Ctrl-C clears typed input, Ctrl-C at an empty prompt exits. Two
# rounds, then the tmux session is killed.
agent_opencode_exit() { # tmux-session pane id
    local attempt
    for attempt in 1 2; do
        tmux send-keys -t "$2" Escape Escape
        sleep 0.2
        tmux send-keys -t "$2" C-c
        sleep 0.3
        tmux send-keys -t "$2" C-c
        if claude_left "$1" "$2" 3; then
            # A failed exit leaves the start's epilogue waiting for a key.
            if tmux has-session -t "=$1" 2>/dev/null; then tmux kill-session -t "=$1" 2>/dev/null || true; fi
            return 0
        fi
    done
    echo "claude-sessions: opencode in $1 did not exit, killing its tmux session" >&2
    tmux kill-session -t "=$1"
}

# Its screen is not Claude's: the daemon's state, from its plugin, says whether it waits.
agent_opencode_say() { # id pane message
    local id=$1 pane=$2 message=$3
    if [[ $(session_field "$id" state) != waiting ]]; then
        echo "claude-sessions: $(session_field "$id" project) is not waiting at its prompt; m queues the message for its turn's end" >&2
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
