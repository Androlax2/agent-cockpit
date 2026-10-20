# Native Codex CLI; all execution stays in its own greywall launcher.
# shellcheck shell=bash

CODEX_LAUNCHER="${CLAUDE_SESSIONS_CODEX:-$HOME/.local/bin/greywall-agents/codex}"

agent_codex_label() { echo "Codex CLI"; }
agent_codex_is_session_id() { [[ $1 =~ ^cdx_[0-9a-f]{64}$ ]]; }
agent_codex_tmux_prefix() { echo "claude-retry-codex-"; }
agent_codex_can() { return 1; }

codex_launch() { # directory native-arguments...
    local directory=$1 name size
    shift
    require_plain_directory "$directory" || return
    name="claude-retry-codex-$(date +%s%N)"
    size=$(detached_size)
    tmux new-session -d -s "$name" -x "${size% *}" -y "${size#* }" -c "$directory" \
        env -u CLAUDE_CODE_SESSION_ID CLAUDE_SESSIONS_INSIDE=1 CLAUDE_SESSIONS_AGENT=codex \
        sh -c '"$@"; result=$?; if [ "$result" -ne 0 ]; then claude-sessions log manager ERROR "native Codex exited with status $result in $PWD"; fi; exit "$result"' \
        codex "$CODEX_LAUNCHER" "$@" || return
    printf '%s\n' "$name"
}

codex_native_thread() { # cockpit-id
    local id=$1 native digest
    agent_codex_is_session_id "$id" || { echo "claude-sessions: invalid native Codex cockpit id" >&2; return 1; }
    native=$(session_field "$id" native_thread 2>/dev/null || true)
    if [[ -z $native && -f $RECENT_SESSIONS ]]; then
        native=$(jq -er --arg id "$id" '.[$id].native_thread // empty' "$RECENT_SESSIONS") || return 1
    fi
    if [[ ! $native =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]]; then
        echo "claude-sessions: no verified native Codex thread for $id" >&2
        return 1
    fi
    digest=$(printf '%s' "$native" | sha256sum) || return
    if [[ $id != "cdx_${digest%% *}" ]]; then
        echo "claude-sessions: native Codex thread mapping does not match $id" >&2
        return 1
    fi
    printf '%s\n' "$native"
}

agent_codex_new() { # directory native-arguments...
    local directory=$1 target
    shift
    target=$(codex_launch "$directory" "$@") || return
    view_session "$target"
    label_panes "$(basename "$directory")"
}
agent_codex_new_with_prompt() { agent_codex_new "$1" "$2"; }
agent_codex_background() { codex_launch "$1" "$2" >/dev/null; }
agent_codex_detached() { codex_launch "$1" "$2" >/dev/null; }

agent_codex_resume() { # directory cockpit-id
    local native target
    native=$(codex_native_thread "$2") || return
    target=$(codex_launch "$1" resume "$native") || return
    view_session "$target"
    label_panes "$(basename "$1")"
    focus_viewer
}

agent_codex_restore() { # directory cockpit-id
    local native
    native=$(codex_native_thread "$2") || return
    codex_launch "$1" resume "$native" >/dev/null
}

agent_codex_restart() { # cockpit-id directory effort detached
    local native target
    [[ -z ${3:-} ]] || { echo "claude-sessions: native Codex effort changes are not supported" >&2; return 1; }
    native=$(codex_native_thread "$1") || return
    exit_session "$1" || return
    if [[ ${4:-} == detached ]]; then
        codex_launch "$2" resume "$native" >/dev/null
    else
        target=$(codex_launch "$2" resume "$native") || return
        view_session "$target"
        label_panes "$(basename "$2")"
    fi
}

agent_codex_exit() { # managed-tmux-session pane cockpit-id
    [[ $1 =~ ^claude-retry-codex-[0-9]+$ ]] || { echo "claude-sessions: refusing an unmanaged Codex terminal" >&2; return 1; }
    tmux kill-session -t "=$1"
}

agent_codex_say() {
    echo "claude-sessions: native Codex inbox delivery is not connected to this runtime" >&2
    return 1
}

agent_codex_prompt_start() {
    return 1
}
