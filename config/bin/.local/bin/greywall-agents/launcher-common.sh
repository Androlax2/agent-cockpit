# What every agent's launcher (claude, opencode) prepares before greywall starts, sourced by them:
# the guards, the per-project paths and secrets and the forwards, collected in greywall_options.
# Not executable: this folder is on PATH. The launcher sets $agent (messages) before sourcing it,
# and $own_folders: the denyRead entries of greywall.json that are its agent's own data.

# Chrome writes its crash reports under the config dir even headless; give it one the sandbox allows.
export CHROME_CONFIG_HOME="$HOME/.cache/chrome-sandbox"
mkdir -p "$CHROME_CONFIG_HOME"
# The report socket's folder (osd-report.socket), mounted whole into the sandbox (greywall.json),
# so a socket made again is seen at once: greywall mounts only what exists when it starts.
mkdir -p "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/claude-report"
# The audit log (claude-audit; security audit 2026-10-19, M6): greywall.json hides it from the
# sandbox, which greywall does only for a folder that exists when the sandbox starts.
mkdir -p -m 700 "${XDG_STATE_HOME:-$HOME/.local/state}/claude-audit"
# The phone page's paired devices (claude-remote): hidden the same way.
mkdir -p -m 700 "${XDG_STATE_HOME:-$HOME/.local/state}/claude-remote"
# Created before the sandbox mounts its denyRead entries, even when no FigCraft has started yet.
mkdir -p -m 700 "$HOME/.config/greywall-agents/figcraft"

# ~/dotfiles is the live checkout: stowed into ~/.local/bin and ~/.config, built and run by the host.
# A session writing there would run its code with Theo's rights, unreviewed (security audit
# 2026-10-19, C1). Sessions work in the clone ~/IdeaProjects/dotfiles; claude-sessions
# apply-dotfiles brings its commits over after Theo read the diff.
live_dotfiles=$(cd "$HOME/dotfiles" 2>/dev/null && pwd -P)
case "$(pwd -P)/" in
    "$live_dotfiles"/*)
        echo "$agent launcher: no session in $live_dotfiles, the live checkout the host runs." >&2
        echo "Work in ~/IdeaProjects/dotfiles (make -C ~/dotfiles dotfiles-work creates it)." >&2
        exit 1 ;;
esac

project_root=$(git rev-parse --show-toplevel 2>/dev/null || pwd -P)
project_root=$(realpath -e -- "$project_root") || exit 1
# A parallel clone (claude-sessions' +, <clones>/<repo>/<name>) is its repository's project: same
# GitHub token, same name everywhere.
clones_dir="${CLAUDE_SESSIONS_CLONES:-$HOME/IdeaProjects/.parallel}"
case "$project_root" in
    "$clones_dir"/*/*) project_name=$(basename "$(dirname "$project_root")") ;;
    *) project_name=$(basename "$project_root") ;;
esac
greywall_options="--allow-path $CHROME_CONFIG_HOME"

# A folder's name the way Claude names its folder in ~/.claude/projects: every character but a
# letter or a digit becomes "-".
folder_slug() {
    printf '%s' "$1" | sed 's/[^A-Za-z0-9]/-/g'
}

# ~/.claude is writable (Claude keeps its state there), and ~/.claude/projects holds every project's
# memory and transcripts: a session could plant "memory" another project loads as Theo's own
# (security audit 2026-10-19, H2). Every project's folder but this session's own goes read-only,
# through a settings file of its own: greywall.json plus those folders in denyWrite. Greywall
# protects only folders that exist when the sandbox starts, so every project's and clone's folder
# is made first; a project created later is the gap (a new clone sets aside memory found there).
claude_projects="$HOME/.claude/projects"
own_slug=$(folder_slug "$(pwd -P)")
mkdir -p "$claude_projects/$own_slug"
for folder in "$HOME"/IdeaProjects/*/ "$clones_dir"/*/*/; do
    [ -d "$folder" ] && mkdir -p "$claude_projects/$(folder_slug "${folder%/}")"
done
other_projects=$(for folder in "$claude_projects"/*/; do
    [ -d "$folder" ] && [ "${folder%/}" != "$claude_projects/$own_slug" ] && printf '%s\n' "${folder%/}"
done)
settings_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/greywall-agents"
settings="$settings_dir/$own_slug.$agent.json"
mkdir -p "$settings_dir"
# greywall.json hides some agents' data from every sandbox (opencode's database and credentials
# from Claude sessions). Hidden, it is an empty folder mounted over the one the agent's profile
# allows, last, so the agent's own sessions lost what they wrote (opencode's sessions, on
# 2026-10-19): its own entries leave denyRead in its settings.
if ! jq --arg others "$other_projects" --arg own "${own_folders:-}" \
        '.filesystem.denyWrite += ($others | split("\n") | map(select(. != "")))
         | .filesystem.denyWrite = ((.filesystem.denyWrite + ["~/.config/greywall-agents/figcraft"]) | unique)
         | .filesystem.denyRead -= ($own | split(" ") | map(select(. != "")))
         | .filesystem.denyRead = ((.filesystem.denyRead + ["~/.config/greywall-agents/figcraft"]) | unique)' \
        "$HOME/.config/greywall/greywall.json" > "$settings.new"; then
    echo "$agent launcher: cannot write $settings from ~/.config/greywall/greywall.json, not starting." >&2
    exit 1
fi
mv "$settings.new" "$settings"
greywall_options="$greywall_options --settings $settings"

# Theo's answers to this project's ask-to-run and ask-to-guide requests, written outside the
# sandbox. One folder per project, in the runtime directory, which no sandbox sees: only this
# project's folder is mounted, read-only, so no session reads another project's command output or
# guide answers (security audit 2026-10-19, M3). ask-to-run names it in its requests.
answers_key=$(folder_slug "$project_root")
answers_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/claude-answers/$answers_key"
mkdir -p "$answers_dir/runs" "$answers_dir/guides"
export CLAUDE_ANSWERS_KEY="$answers_key"
greywall_options="$greywall_options --allow-read-path $answers_dir"

# The work clone and its parallel clones (<clones>/dotfiles/<name>) fetch from the live checkout
# (their origin) to start from what is applied, and the session hook compares their commits with
# it for the card's "↑N unapplied". Read-only.
case "$project_root" in
    "$HOME/IdeaProjects/dotfiles"|"$clones_dir"/dotfiles/*) is_dotfiles_clone=true ;;
    *) is_dotfiles_clone=false ;;
esac
if [ -n "$live_dotfiles" ] && "$is_dotfiles_clone"; then
    greywall_options="$greywall_options --allow-read-path $live_dotfiles"
fi

# Greyproxy re-signs HTTPS with a CA the system deliberately does not trust. Greywall hands the
# sandbox that CA alone (SSL_CERT_FILE), which git, curl, cargo and PHP ignore, so they get a bundle of
# the system roots plus Greyproxy's CA. Rebuilt on every start so a regenerated CA is picked up;
# ~/.cache/agent-ca is readable but not writable inside the sandbox.
ca_dir="$HOME/.cache/agent-ca"
mkdir -p "$ca_dir/php"
cat /etc/ssl/cert.pem "$HOME/.local/share/greyproxy/ca-cert.pem" > "$ca_dir/bundle.pem.new"
mv "$ca_dir/bundle.pem.new" "$ca_dir/bundle.pem"
printf 'openssl.cafile=%s\ncurl.cainfo=%s\n' "$ca_dir/bundle.pem" "$ca_dir/bundle.pem" > "$ca_dir/php/zz-agent-ca.ini"
export GIT_SSL_CAINFO="$ca_dir/bundle.pem"
export CURL_CA_BUNDLE="$ca_dir/bundle.pem"
export CARGO_HTTP_CAINFO="$ca_dir/bundle.pem"
# The leading ":" keeps PHP's own scan directory and adds ours after it.
export PHP_INI_SCAN_DIR=":$ca_dir/php"

# Image paste: the Wayland socket stays hidden from the sandbox. A stand-in wl-paste asks the
# clipboard-image bridge (a user service serving PNG images only). The stand-in lives in the
# dotfiles, which the sandbox cannot see, so it is copied to ~/.cache/agent-shims (readable, not
# writable inside the sandbox) on every start and put first on the sandbox's PATH only.
shims_dir="$HOME/.cache/agent-shims/bin"
mkdir -p "$shims_dir"
cp "$HOME/.local/share/agent-shims/bin/wl-paste" "$shims_dir/wl-paste.new"
chmod 755 "$shims_dir/wl-paste.new"
mv "$shims_dir/wl-paste.new" "$shims_dir/wl-paste"
export PATH="$shims_dir:$PATH"
clipboard_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/clipboard-image"
if [ -d "$clipboard_dir" ]; then
    greywall_options="$greywall_options --allow-path $clipboard_dir"
fi

# A per-project GitHub token from the keyring (make agent-github-token PROJECT=<name>).
# Without one, gh inside the sandbox stays logged out: the keyring itself is hidden from it.
# It is passed in as-is, not as a greyproxy placeholder: git sends credentials Basic-encoded,
# where greyproxy cannot swap a placeholder, so git push needs the real, repo-scoped token.
github_token=$(secret-tool lookup service agent-gh project "$project_name" 2>/dev/null || true)
if [ -n "$github_token" ]; then
    export GH_TOKEN="$github_token"
    greywall_options="$greywall_options --ignore-secret GH_TOKEN"
fi

# A classic token with only the project scope (Project boards, which fine-grained tokens cannot
# reach), shared by every project. Unlike GH_TOKEN it stays a greyproxy placeholder inside the
# sandbox: gh sends it in an Authorization header, where greyproxy swaps in the real value.
project_board_token=$(secret-tool lookup service agent-gh-projects 2>/dev/null || true)
if [ -n "$project_board_token" ]; then
    export GH_PROJECT_TOKEN="$project_board_token"
    greywall_options="$greywall_options --secret GH_PROJECT_TOKEN"
fi

# The Console's API key only goes to a credits session (the claude launcher sets it after this);
# every other session gets none, whatever the calling environment holds.
unset ANTHROPIC_API_KEY

# A project with its own isolated Docker daemon (make agent-docker-add PROJECT=<name>) has its
# socket at the project root. Started in a subfolder, greywall mounts only that folder, so the
# socket needs a mount of its own; at the project root the folder mount already includes it and
# stays live when the relay recreates the socket, which a single-file mount would not.
docker_socket="$project_root/.agent-docker.sock"
if [ -S "$docker_socket" ]; then
    export DOCKER_HOST="unix://$docker_socket"
    # Build steps and containers live in the daemon's network namespace, where greyproxy is
    # 10.0.2.2, not the 127.0.0.1 the daemon itself uses. The client injects this proxy into
    # both, as build arguments and as container environment.
    docker_config="$HOME/.cache/agent-docker-config/$project_name"
    container_proxy="http://$project_name-docker:proxy@10.0.2.2:43051"
    mkdir -p "$docker_config"
    printf '{"proxies":{"default":{"httpProxy":"%s","httpsProxy":"%s","noProxy":"localhost,127.0.0.1"}}}\n' \
        "$container_proxy" "$container_proxy" > "$docker_config/config.json"
    export DOCKER_CONFIG="$docker_config"
    greywall_options="$greywall_options --allow-path $docker_config"
    if [ "$(pwd -P)" != "$project_root" ]; then
        greywall_options="$greywall_options --allow-path $docker_socket"
    fi
fi

# After a reboot a session can start before DNS (Tailscale included) is up, and the sandbox's
# DNS link to greyproxy never recovers. Wait until greyproxy's resolver answers, up to 30s.
wait_for_greyproxy_dns() {
    attempts=0
    until dig +short +time=1 +tries=1 @127.0.0.1 -p 43053 api.anthropic.com | grep -q .; do
        attempts=$((attempts + 1))
        if [ "$attempts" -ge 30 ]; then
            echo "$agent launcher: no DNS through greyproxy after 30s, starting anyway" >&2
            return
        fi
        sleep 1
    done
}

wait_for_greyproxy_dns

# Never inherit another checkout's connection. The host reservation, exact automation roots and
# private recovery directory must all agree before any port enters the sandbox (audit M2).
if ! FIGCRAFT_CONTROL_PORT=$(bash "$(dirname "$0")/figcraft.sh" "$project_root"); then
    echo "$agent launcher: cannot determine this checkout's FigCraft connection" >&2
    exit 1
fi
export FIGCRAFT_CONTROL_PORT
if [ -n "$FIGCRAFT_CONTROL_PORT" ]; then
    greywall_options="$greywall_options --forward $FIGCRAFT_CONTROL_PORT"
fi

# Host ports forwarded into this project's sandboxes, for a limited time.
# One file per project, kept outside the sandbox so a session cannot widen
# its own access. Content: "<expiry epoch> <port>...", e.g.
#   ~/.config/greywall-agents/forward/opusline  ->  1760817600 3000 8088 8025
# A running session keeps its forwards until it exits; the expiry only stops
# new sessions from getting them.
forward_root=$(git -C "$PWD" rev-parse --show-toplevel 2>/dev/null || printf '%s' "$PWD")
forward_file="$HOME/.config/greywall-agents/forward/$(basename "$forward_root")"
if [ -r "$forward_file" ]; then
    read -r forward_until forward_ports < "$forward_file"
    if [ "$(date +%s)" -lt "$forward_until" ]; then
        for port in $forward_ports; do
            reservation="$HOME/.config/greywall-agents/figcraft/ports/$port"
            if [ -f "$reservation" ] && [ "$(cat "$reservation")" != "$project_root" ]; then
                echo "$agent launcher: port $port in $forward_file is reserved for another checkout's FigCraft; remove that grant before restarting." >&2
                exit 1
            fi
            case " $greywall_options " in
                *" --forward $port "*) ;;  # already forwarded above
                *) greywall_options="$greywall_options --forward $port" ;;
            esac
        done
    else
        echo "greywall-agents: port forwards in $forward_file expired, not applied" >&2
    fi
fi
