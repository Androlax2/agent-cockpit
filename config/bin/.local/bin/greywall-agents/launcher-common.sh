# What every agent's launcher (claude, opencode) prepares before greywall starts, sourced by them:
# the guards, the per-project paths and secrets and the forwards, collected in greywall_options.
# Not executable: this folder is on PATH. The launcher sets $agent (messages) before sourcing it,
# and $own_folders: the denyRead entries of greywall.json that are its agent's own data.

# Greywall's drift prompt defaults to merging bundled grants into our reviewed profiles.
export GREYWALL_NO_PROMPT=1

# Chrome writes its crash reports under the config dir even headless; give it one the sandbox allows.
export CHROME_CONFIG_HOME="$HOME/.cache/chrome-sandbox"
mkdir -p "$CHROME_CONFIG_HOME"
# The report socket's folder (osd-report.socket), mounted whole and read-only into the sandbox
# (greywall.json), so a socket made again is seen at once and no session can replace it: greywall
# mounts only what exists when it starts.
prepare_cockpit_folder() {
    if [ -d "$1" ]; then
        printf '%s\n' "$1"
    elif [ -d "$2" ]; then
        printf '%s\n' "$2"
    else
        mkdir -p -m 700 "$1" || return 1
        mkdir -p "$(dirname "$2")" || return 1
        [ -L "$2" ] || ln -s "$1" "$2" || return 1
        printf '%s\n' "$1"
    fi
}
runtime_root="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
state_root="${COCKPIT_STATE_HOME:-${XDG_STATE_HOME:-$HOME/.local/state}}"
prepare_cockpit_folder "$runtime_root/cockpit-report" "$runtime_root/claude-report" >/dev/null || exit 1
# The audit log (claude-audit; security audit 2026-10-19, M6): greywall.json hides it from the
# sandbox, which greywall does only for a folder that exists when the sandbox starts.
mkdir -p -m 700 "${XDG_STATE_HOME:-$HOME/.local/state}/claude-audit"
# Both spellings stay hidden during the transition, including fresh sandboxes.
prepare_cockpit_folder "$state_root/cockpit-remote" "$state_root/claude-remote" >/dev/null || exit 1
# What sessions that ignored their exit showed (cockpit record_stuck_exit): hidden the same way.
prepare_cockpit_folder "$state_root/cockpit-stuck-exits" "$state_root/claude-stuck-exits" >/dev/null || exit 1
# Created before the sandbox mounts its denyRead entries, even when no FigCraft has started yet.
mkdir -p -m 700 "$HOME/.config/greywall-agents/figcraft"

# ~/dotfiles, ~/agent-cockpit and ~/vitals are running copies: linked into ~/.local/bin and
# ~/.config, built and run by the host. A session writing there would run its code with Theo's
# rights, unreviewed (security audit 2026-10-19, C1). Sessions work in a clone,
# ~/IdeaProjects/<name>; cockpit apply brings its commits over after Theo read the diff.
reviewed_repositories="dotfiles agent-cockpit vitals"
for reviewed in $reviewed_repositories; do
    running_copy=$(cd "$HOME/$reviewed" 2>/dev/null && pwd -P) || continue
    case "$(pwd -P)/" in
        "$running_copy"/*)
            echo "$agent launcher: no session in $running_copy, the running copy the host runs." >&2
            echo "Work in its clone, ~/IdeaProjects/$reviewed." >&2
            exit 1 ;;
    esac
done

refuse_git_metadata() {
    echo "$agent launcher: unsupported Git metadata for $project_root: $1" >&2
    echo "Use a self-contained clone (git clone --no-local <source> <destination>), not an external worktree or gitdir. No extra access was granted." >&2
    exit 1
}

if project_root=$(git rev-parse --show-toplevel 2>/dev/null); then
    has_git_repository=true
else
    has_git_repository=false
    project_root=$(pwd -P)
    if [ -n "${GIT_DIR:-}${GIT_COMMON_DIR:-}${GIT_WORK_TREE:-}" ]; then
        refuse_git_metadata "cannot resolve the repository selected by the Git environment"
    fi
    metadata_parent=$project_root
    while [ "$metadata_parent" != / ]; do
        if [ -e "$metadata_parent/.git" ] || [ -L "$metadata_parent/.git" ]; then
            refuse_git_metadata "cannot resolve the repository at $metadata_parent/.git"
        fi
        metadata_parent=$(dirname "$metadata_parent")
    done
fi
project_root=$(realpath -e -- "$project_root") || exit 1
# The saved default-deny policy cannot read external metadata; pointers must never add grants.
if "$has_git_repository"; then
    for metadata_kind in --absolute-git-dir --git-common-dir; do
        metadata_path=$(git -C "$project_root" rev-parse --path-format=absolute "$metadata_kind" 2>/dev/null) \
            && metadata_path=$(realpath -e -- "$metadata_path") \
            || refuse_git_metadata "cannot resolve $metadata_kind"
        case "$metadata_path" in
            "$project_root"/*) ;;
            *) refuse_git_metadata "$metadata_kind resolves outside the checkout: $metadata_path" ;;
        esac
    done
fi
# A parallel clone (cockpit' +, <clones>/<repo>/<name>) is its repository's project: same
# GitHub token, same name everywhere.
clones_dir="${COCKPIT_CLONES:-$HOME/IdeaProjects/.parallel}"
case "$project_root" in
    "$clones_dir"/*/*) project_name=$(basename "$(dirname "$project_root")") ;;
    *) project_name=$(basename "$project_root") ;;
esac
greywall_options="--allow-path $CHROME_CONFIG_HOME"
for cache_name in runs designs; do
    cockpit_cache=$(prepare_cockpit_folder "$HOME/.cache/cockpit/$cache_name" "$HOME/.cache/claude/$cache_name") || exit 1
    greywall_options="$greywall_options --allow-path $cockpit_cache"
done

# Greywall's private /tmp keeps Go verification writable without exposing the host's build
# or module caches to writes or letting a session poison artifacts used outside the sandbox.
export GOCACHE=/tmp/greywall-go-build
export GOMODCACHE=/tmp/greywall-go-mod

# A folder's name the way Claude names its folder in ~/.claude/projects: every character but a
# letter or a digit becomes "-".
folder_slug() {
    printf '%s' "$1" | sed 's/[^A-Za-z0-9]/-/g'
}

# greyproxy shows and matches this sandbox's requests by its login: <agent>-<project> says which
# project asks for a host. A label, not proof: the password is greywall's fixed "proxy", so any
# sandbox can log in under any name. Never scope an approval to it as a security boundary.
greywall_options="$greywall_options --proxy-user $agent-$(folder_slug "$project_name")"

# ~/.claude is writable (Claude keeps its state there), and ~/.claude/projects holds every project's
# memory and transcripts: a session could plant "memory" another project loads as Theo's own
# (security audit 2026-10-19, H2). Every project's folder but this session's own goes read-only,
# through a settings file of its own: greywall.json plus those folders in denyWrite. Greywall
# protects only folders that exist when the sandbox starts, so every project's and clone's folder
# is made first; a project created later is the gap (a new clone sets aside memory found there).
#
# Greywall hands bwrap all its rules as one argument, which Linux caps at 128 KiB, and a rule per
# folder ever made passed it on 2026-10-25 ("argument list too long"). The folder of a project or
# clone that is gone gets no rule: a fresh clone sets aside memory found in its folder. A folder
# outside IdeaProjects and the clones, or one an existing checkout may own (a subfolder's session),
# keeps its rule.
claude_projects="$HOME/.claude/projects"
own_slug=$(folder_slug "$(pwd -P)")
mkdir -p "$claude_projects/$own_slug"
checkout_slugs=""
for folder in "$HOME"/IdeaProjects/*/ "$clones_dir"/*/*/; do
    [ -d "$folder" ] || continue
    checkout_slug=$(folder_slug "${folder%/}")
    mkdir -p "$claude_projects/$checkout_slug"
    checkout_slugs="$checkout_slugs$checkout_slug
"
done
other_projects=$(for folder in "$claude_projects"/*/; do
    [ -d "$folder" ] && [ "${folder%/}" != "$claude_projects/$own_slug" ] && printf '%s\n' "${folder%/}"
done | awk -v checkouts="$checkout_slugs" -v projects="$(folder_slug "$HOME/IdeaProjects")-" \
        -v clones="$(folder_slug "$clones_dir")-" '
    BEGIN { split(checkouts, owners, "\n") }
    {
        slug = $0
        sub(/.*\//, "", slug)
        if (index(slug, projects) != 1 && index(slug, clones) != 1) { print; next }
        for (i in owners) {
            if (owners[i] != "" && (slug == owners[i] || index(slug, owners[i] "-") == 1)) { print; next }
        }
    }') || {
    echo "$agent launcher: cannot list the other projects' folders in $claude_projects, not starting." >&2
    exit 1
}
settings_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/greywall-agents"
settings="$settings_dir/$own_slug.$agent.json"
mkdir -p "$settings_dir"
# Each launch stages its files under a name of its own before moving them into place: the restore
# after login starts every session at once, and shared ".new" names made them collide (2026-10-25:
# "mv: cannot stat …/bundle.pem.new").
install_staged() { # staged-file target mode
    if ! chmod "$3" "$1" || ! mv "$1" "$2"; then
        rm -f "$1"
        echo "$agent launcher: cannot install $2, not starting." >&2
        exit 1
    fi
}
# greywall.json hides some agents' data from every sandbox (opencode's database and credentials
# from Claude sessions). Hidden, it is an empty folder mounted over the one the agent's profile
# allows, last, so the agent's own sessions lost what they wrote (opencode's sessions, on
# 2026-10-19): its own entries leave denyRead in its settings.
#
# greywall.json's ".env.*" hides every match at the top of the folder greywall starts in, committed
# templates too (.env.production.example: opusline's e2e stack could not start, 2026-10-21). A
# file git tracks and the work tree leaves unchanged hides nothing: the session reads it with git
# show. The glob becomes the paths of every other match; greywall's built-in list still hides
# .env, .env.local, .env.production... whatever git says. A name the list cannot carry keeps the glob.
hidden_env_files=""
keeps_env_glob=false
for env_file in "$PWD"/.env.*; do
    [ -e "$env_file" ] || [ -L "$env_file" ] || continue
    if [ -f "$env_file" ] && [ ! -L "$env_file" ] \
        && git --literal-pathspecs ls-files --error-unmatch -- "$env_file" >/dev/null 2>&1 \
        && git --literal-pathspecs diff --quiet HEAD -- "$env_file" >/dev/null 2>&1; then
        continue
    fi
    case "$env_file" in
        *"
"*) keeps_env_glob=true ;;
    esac
    hidden_env_files="$hidden_env_files$env_file
"
done
staged_settings=$(mktemp "$settings.XXXXXX") || exit 1
if ! jq --arg others "$other_projects" --arg own "${own_folders:-}" \
        --arg hidden_env "$hidden_env_files" --argjson keeps_env_glob "$keeps_env_glob" \
        '.filesystem.denyWrite += ($others | split("\n") | map(select(. != "")))
         | .filesystem.denyWrite = ((.filesystem.denyWrite + ["~/.config/greywall-agents/figcraft"]) | unique)
         | .filesystem.denyRead -= ($own | split(" ") | map(select(. != "")))
         | .filesystem.denyRead = ((.filesystem.denyRead + ["~/.config/greywall-agents/figcraft"]) | unique)
         | if $keeps_env_glob or (any(.filesystem.denyRead[]; . == ".env.*") | not) then .
           else .filesystem.denyRead = (.filesystem.denyRead - [".env.*"] + ($hidden_env | split("\n") | map(select(. != "")))) end' \
        "$HOME/.config/greywall/greywall.json" > "$staged_settings"; then
    rm -f "$staged_settings"
    echo "$agent launcher: cannot write $settings from ~/.config/greywall/greywall.json, not starting." >&2
    exit 1
fi
install_staged "$staged_settings" "$settings" 644
greywall_options="$greywall_options --settings $settings"

# Theo's answers to this project's ask-to-run and ask-to-guide requests, written outside the
# sandbox. One folder per project, in the runtime directory, which no sandbox sees: only this
# project's folder is mounted, read-only, so no session reads another project's command output or
# guide answers (security audit 2026-10-19, M3). ask-to-run names it in its requests.
answers_key=$(folder_slug "$project_root")
answers_root=$(prepare_cockpit_folder "$runtime_root/cockpit-answers" "$runtime_root/claude-answers") || exit 1
answers_dir="$answers_root/$answers_key"
mkdir -p "$answers_dir/runs" "$answers_dir/guides"
export COCKPIT_ANSWERS_KEY="$answers_key"
# Older agent binaries inside an already-running sandbox still accept this name.
export CLAUDE_ANSWERS_KEY="$answers_key"
greywall_options="$greywall_options --allow-read-path $answers_dir"
# Linked sessions (cockpit-team): the daemon's host-only team socket, hard-linked into this
# project's read-only answers folder. No new mount; a session sees no other project's link.
if [ -x "$HOME/.local/bin/cockpit-team" ] && ! "$HOME/.local/bin/cockpit-team" prepare; then
    echo "$agent launcher: linked-session messaging unavailable until the OSD daemon runs" >&2
fi

# A reviewed repository's work clone and its parallel clones (<clones>/<name>/<clone>) fetch from
# the running copy (their origin) to start from what is applied, and the session hook compares
# their commits with it for the card's "↑N unapplied". Read-only.
for reviewed in $reviewed_repositories; do
    running_copy=$(cd "$HOME/$reviewed" 2>/dev/null && pwd -P) || continue
    case "$project_root" in
        "$HOME/IdeaProjects/$reviewed"|"$clones_dir/$reviewed"/*)
            greywall_options="$greywall_options --allow-read-path $running_copy" ;;
    esac
done

# Greyproxy re-signs HTTPS with a CA the system deliberately does not trust. Greywall hands the
# sandbox that CA alone (SSL_CERT_FILE), which git, curl, cargo and PHP ignore, so they get a bundle of
# the system roots plus Greyproxy's CA. Rebuilt on every start so a regenerated CA is picked up;
# ~/.cache/agent-ca is readable but not writable inside the sandbox.
ca_dir="$HOME/.cache/agent-ca"
mkdir -p "$ca_dir/php"
staged_bundle=$(mktemp "$ca_dir/bundle.pem.XXXXXX") || exit 1
cat /etc/ssl/cert.pem "$HOME/.local/share/greyproxy/ca-cert.pem" > "$staged_bundle"
install_staged "$staged_bundle" "$ca_dir/bundle.pem" 644
printf 'openssl.cafile=%s\ncurl.cainfo=%s\n' "$ca_dir/bundle.pem" "$ca_dir/bundle.pem" > "$ca_dir/php/zz-agent-ca.ini"
export GIT_SSL_CAINFO="$ca_dir/bundle.pem"
export CURL_CA_BUNDLE="$ca_dir/bundle.pem"
export CARGO_HTTP_CAINFO="$ca_dir/bundle.pem"
# The leading ":" keeps PHP's own scan directory and adds ours after it.
export PHP_INI_SCAN_DIR=":$ca_dir/php"

# The sandbox cannot read ~/.gitconfig (a link into the hidden dotfiles), so commits there had no
# author. Hand over the identity the host's git uses for this checkout, two values and no mount;
# without one, none: git then refuses to commit instead of using an inherited or guessed author.
unset GIT_AUTHOR_NAME GIT_AUTHOR_EMAIL GIT_COMMITTER_NAME GIT_COMMITTER_EMAIL
git_user_name=$(git -C "$project_root" config --get user.name 2>/dev/null) || git_user_name=
git_user_email=$(git -C "$project_root" config --get user.email 2>/dev/null) || git_user_email=
if [ -n "$git_user_name" ] && [ -n "$git_user_email" ]; then
    export GIT_AUTHOR_NAME="$git_user_name" GIT_COMMITTER_NAME="$git_user_name"
    export GIT_AUTHOR_EMAIL="$git_user_email" GIT_COMMITTER_EMAIL="$git_user_email"
else
    echo "$agent launcher: no git user.name and user.email on the host; commits in this session will fail." >&2
fi

# Image paste: the Wayland socket stays hidden from the sandbox. A stand-in wl-paste asks the
# clipboard-image bridge (a user service serving PNG images only). The stand-in lives in the
# dotfiles, which the sandbox cannot see, so it is copied to ~/.cache/agent-shims (readable, not
# writable inside the sandbox) on every start and put first on the sandbox's PATH only.
shims_dir="$HOME/.cache/agent-shims/bin"
mkdir -p "$shims_dir"
staged_paste=$(mktemp "$shims_dir/wl-paste.XXXXXX") || exit 1
cp "$HOME/.local/share/agent-shims/bin/wl-paste" "$staged_paste"
install_staged "$staged_paste" "$shims_dir/wl-paste" 755
# Only build tools inherit this affinity; the agent itself keeps every available CPU thread.
# All sessions choose the same half, so more parallel builds do not multiply their CPU budget.
if ! AGENT_BUILD_CPUS=$(python3 "$HOME/.local/bin/agent-build" --cpu-list); then
    echo "$agent launcher: cannot determine the shared build CPU budget, not starting." >&2
    exit 1
fi
export AGENT_BUILD_CPUS
build_runner_copy=$(mktemp "$shims_dir/agent-build.XXXXXX") || exit 1
if ! cp "$HOME/.local/bin/agent-build" "$build_runner_copy" || ! chmod 755 "$build_runner_copy" ||
        ! mv "$build_runner_copy" "$shims_dir/agent-build"; then
    rm -f "$build_runner_copy"
    echo "$agent launcher: cannot install the build CPU guard, not starting." >&2
    exit 1
fi
for build_tool in cargo make npm pnpm go composer pytest; do
    ln -sfn agent-build "$shims_dir/$build_tool" || exit 1
done
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
# A parallel clone shares its repository's daemon through a socket in a folder of its own beside
# the clones (agent-docker-clones-relay@), mounted whole: never the repository's checkout.
docker_socket="$project_root/.agent-docker.sock"
docker_socket_mount=
unset AGENT_DOCKER_CA_BUNDLE
case "$project_root" in
    "$clones_dir"/*/*)
        docker_socket="$clones_dir/$project_name/.agent-docker/docker.sock"
        docker_socket_mount=$(dirname "$docker_socket") ;;
    *)
        if [ "$(pwd -P)" != "$project_root" ]; then docker_socket_mount=$docker_socket; fi ;;
esac
if [ -S "$docker_socket" ]; then
    export DOCKER_HOST="unix://$docker_socket"
    docker_build_copy=$(mktemp "$shims_dir/agent-docker-build.XXXXXX") || exit 1
    if ! cp "$HOME/.local/bin/agent-docker-build" "$docker_build_copy" || ! chmod 755 "$docker_build_copy" ||
            ! mv "$docker_build_copy" "$shims_dir/agent-docker-build"; then
        rm -f "$docker_build_copy"
        echo "$agent launcher: cannot install the Docker build trust helper, not starting." >&2
        exit 1
    fi
    export AGENT_DOCKER_CA_BUNDLE="$ca_dir/bundle.pem"
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
    if [ -n "$docker_socket_mount" ]; then
        greywall_options="$greywall_options --allow-path $docker_socket_mount"
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
# new sessions from getting them. Parallel clones share their repository's file.
forward_file="$HOME/.config/greywall-agents/forward/$project_name"
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
