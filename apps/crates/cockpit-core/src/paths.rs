//! Where everything lives, from the environment like the Python column did: tests and the demo
//! cockpit point HOME, XDG_RUNTIME_DIR and XDG_STATE_HOME elsewhere.

use std::env;
use std::path::PathBuf;

pub fn home() -> PathBuf {
    PathBuf::from(env::var("HOME").unwrap_or_else(|_| "/".into()))
}

/// `~/…` expanded against HOME.
pub fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None => PathBuf::from(path),
    }
}

pub fn runtime_dir() -> PathBuf {
    PathBuf::from(env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into()))
}

/// The filesystems whose fill the column watches: / and home, or COCKPIT_DISK_MOUNTS
/// (colon-separated; empty watches none, as the tests do).
pub fn disk_mounts() -> Vec<String> {
    match env::var("COCKPIT_DISK_MOUNTS") {
        Ok(list) => list
            .split(':')
            .filter(|mount| !mount.is_empty())
            .map(str::to_string)
            .collect(),
        Err(_) => vec!["/".into(), home().display().to_string()],
    }
}

/// COCKPIT_STATE_HOME first: the opencode launcher moves XDG_STATE_HOME into the sandbox's
/// private /tmp for opencode's own state, and every tool opencode runs inherits it; this one keeps
/// the cockpit's tools (claude-todo's list) on the real folder.
pub fn state_home() -> PathBuf {
    env::var("COCKPIT_STATE_HOME")
        .or_else(|_| env::var("XDG_STATE_HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/state"))
}

fn from_env(name: &str, default: PathBuf) -> PathBuf {
    env::var(name).map(PathBuf::from).unwrap_or(default)
}

/// New configuration wins; running sessions may still carry the pre-rename override.
pub fn cockpit_env(canonical: &str, legacy: &str) -> Result<String, env::VarError> {
    match env::var(canonical) {
        Err(env::VarError::NotPresent) => env::var(legacy),
        value => value,
    }
}

/// Before host migration, use existing data rather than start an empty second store.
pub fn renamed_path(canonical: PathBuf, legacy: PathBuf) -> PathBuf {
    if !canonical.exists() && legacy.exists() {
        legacy
    } else {
        canonical
    }
}

pub fn cockpit_runtime(name: &str, legacy: &str) -> PathBuf {
    renamed_path(runtime_dir().join(name), runtime_dir().join(legacy))
}

pub fn cockpit_cache(name: &str) -> PathBuf {
    renamed_path(
        home().join(".cache/cockpit").join(name),
        home().join(".cache/claude").join(name),
    )
}

pub fn state_dir() -> PathBuf {
    state_home().join("cockpit")
}
pub fn snapshot() -> PathBuf {
    runtime_dir().join("cockpit.json")
}
/// cockpit' loading screen state while a session starts or restarts in the viewer pane.
pub fn loading_state() -> PathBuf {
    runtime_dir().join("cockpit-loading")
}
pub fn current() -> PathBuf {
    runtime_dir().join("cockpit.current")
}
/// One note per session `cockpit restart` is bringing back (sessions::restarts).
pub fn restarting_dir() -> PathBuf {
    runtime_dir().join("cockpit.restarting")
}
/// While it exists, the phone page notifies: the lock screen's cockpit away on.
pub fn phone_away() -> PathBuf {
    cockpit_runtime("cockpit-remote-away", "claude-remote-away")
}
pub fn usage_cache() -> PathBuf {
    runtime_dir().join("claude-usage.json")
}
pub fn codex_usage_cache() -> PathBuf {
    runtime_dir().join("codex-usage.json")
}
pub fn codex_session_script() -> PathBuf {
    home().join(".config/waybar/scripts/codex-session.py")
}
/// Sessions' reports and requests: the one FIFO the sandboxes can write (greywall allowWrite).
pub fn osd_fifo() -> PathBuf {
    runtime_dir().join("osd.fifo")
}
/// Everything else the OSD daemon takes (banners, focus, clearing, phases, volume): only programs
/// outside the sandbox reach this one.
pub fn osd_host_fifo() -> PathBuf {
    runtime_dir().join("osd-host.fifo")
}
/// Session reports: a socket (osd-report.socket keeps it across daemon restarts), so the daemon
/// learns which process sent each report and takes the pane from where it really runs.
/// In a folder of its own, which the sandboxes mount whole: a socket file mounted alone stays the
/// old one in every running sandbox once the socket is made again.
pub fn osd_report_socket() -> PathBuf {
    cockpit_runtime("cockpit-report", "claude-report").join("osd-report.sock")
}
/// The Claude sessions' scopes game-performance froze for a game, one per line; gone after it.
pub fn game_frozen() -> PathBuf {
    runtime_dir().join("game-performance.frozen")
}
pub fn failsafe_state() -> PathBuf {
    runtime_dir().join("cockpit-failsafe.json")
}
pub fn history() -> PathBuf {
    from_env("COCKPIT_HISTORY", home().join(".claude/history.jsonl"))
}
pub fn transcripts() -> PathBuf {
    from_env("COCKPIT_TRANSCRIPTS", home().join(".claude/projects"))
}
/// opencode's sessions and messages ($XDG_DATA_HOME/opencode/opencode.db), which its sandboxes write.
pub fn opencode_database() -> PathBuf {
    let data_home = env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share"));
    from_env(
        "COCKPIT_OPENCODE_DB",
        data_home.join("opencode/opencode.db"),
    )
}
pub fn statusline_dir() -> PathBuf {
    home().join(".cache/claude")
}
pub fn billing_dir() -> PathBuf {
    statusline_dir().join("billing")
}
pub fn usage_script() -> PathBuf {
    home().join(".config/waybar/scripts/claude-usage.sh")
}
pub fn credentials() -> PathBuf {
    home().join(".claude/.credentials.json")
}
pub fn view_state() -> PathBuf {
    state_dir().join("view.json")
}
pub fn continuity() -> PathBuf {
    state_dir().join("continuity.json")
}
pub fn subscription_routes() -> PathBuf {
    state_dir().join("subscriptions/routes.json")
}
pub fn usage_archive() -> PathBuf {
    state_dir().join("usage.sqlite")
}
pub fn alerts() -> PathBuf {
    state_dir().join("alerts.json")
}
pub fn log() -> PathBuf {
    state_dir().join("log")
}
/// When the daily digest last printed (epoch ms), next to the cockpit script's digest-day.
pub fn chief_digest_at() -> PathBuf {
    state_dir().join("chief-digest-at")
}
/// When Theo last saw his habits (epoch ms): habits created after it are new.
pub fn chief_habits_seen_at() -> PathBuf {
    state_dir().join("chief-habits-seen-at")
}
pub fn todo() -> PathBuf {
    state_dir().join("todo.json")
}
pub fn todo_images() -> PathBuf {
    state_dir().join("todo-images")
}
pub fn team_database() -> PathBuf {
    crate::audit::dir().join("teams.sqlite")
}
pub fn team_agent_socket() -> PathBuf {
    cockpit_runtime("cockpit-team-host", "claude-team-host").join("agent.sock")
}
/// Approvals, edits, unlinks: never linked into any sandbox, and the daemon refuses sandboxed peers.
pub fn team_host_socket() -> PathBuf {
    cockpit_runtime("cockpit-team-host", "claude-team-host").join("host.sock")
}
pub fn settings() -> PathBuf {
    state_dir().join("settings.json")
}
/// claude-todo's changes, queued by the OSD daemon with the sender's verified folder: the runtime
/// directory, which no sandbox sees (security audit 2026-10-19, M4).
pub fn todo_updates() -> PathBuf {
    cockpit_env("COCKPIT_TODO_UPDATES", "CLAUDE_TODO_UPDATES")
        .map(PathBuf::from)
        .unwrap_or_else(|_| runtime_dir().join("cockpit-todo-updates.jsonl"))
}
pub fn error_reports() -> PathBuf {
    state_dir().join("error-reports.json")
}
pub fn error_report_dir() -> PathBuf {
    state_dir().join("error-reports")
}
pub fn recent_sessions() -> PathBuf {
    state_dir().join("recent-sessions.json")
}
pub fn ended_sessions() -> PathBuf {
    state_dir().join("ended-sessions")
}
/// One note per session that fell asleep (sleep::Asleep), named by its id; waking removes it.
pub fn asleep_dir() -> PathBuf {
    state_dir().join("asleep")
}
pub fn clones_dir() -> PathBuf {
    from_env("COCKPIT_CLONES", home().join("IdeaProjects/.parallel"))
}
pub fn greyproxy_db() -> PathBuf {
    from_env(
        "COCKPIT_GREYPROXY_DB",
        home().join(".local/share/greyproxy/greyproxy.db"),
    )
}
pub fn designs() -> PathBuf {
    cockpit_env("COCKPIT_DESIGNS_DIR", "CLAUDE_DESIGNS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| cockpit_cache("designs"))
}
pub fn dotfiles() -> PathBuf {
    home().join("dotfiles")
}
/// Where the dotfiles session works: a clone the host never runs or builds from. ~/dotfiles, the
/// stowed checkout, only moves forward through `cockpit apply-dotfiles`, after Theo read the
/// diff (security audit 2026-10-19, C1).
pub fn dotfiles_work() -> PathBuf {
    cockpit_env("COCKPIT_DOTFILES_WORK", "CLAUDE_DOTFILES_WORK")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join("IdeaProjects/dotfiles"))
}
/// Theo's greywall fork, where `claude-todo stack greywall` files problems in greywall's own code.
/// The repositories whose running copy the host runs with Theo's rights: sessions work in their
/// clones, and the running copy only takes commits through `cockpit apply`, after Theo read them.
pub const REVIEWED_REPOSITORIES: [&str; 3] = ["dotfiles", "agent-cockpit", "vitals"];
pub fn running_copy(repository: &str) -> PathBuf {
    if repository == "dotfiles" {
        dotfiles()
    } else {
        home().join(repository)
    }
}
pub fn work_clone(repository: &str) -> PathBuf {
    if repository == "dotfiles" {
        dotfiles_work()
    } else {
        home().join("IdeaProjects").join(repository)
    }
}
/// Where the agent-cockpit session works: the cockpit's logged errors and every session's blocked
/// reports go there, since the setup they are about (cockpit, launchers, greywall.json, hooks) is its own.
pub fn cockpit_work() -> PathBuf {
    work_clone("agent-cockpit")
}
pub fn greywall_work() -> PathBuf {
    cockpit_env("COCKPIT_GREYWALL_WORK", "CLAUDE_GREYWALL_WORK")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join("IdeaProjects/greywall"))
}
/// Theo's greyproxy fork, where `claude-todo stack greyproxy` files problems in greyproxy's own code.
pub fn greyproxy_work() -> PathBuf {
    cockpit_env("COCKPIT_GREYPROXY_WORK", "CLAUDE_GREYPROXY_WORK")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join("IdeaProjects/greyproxy"))
}
pub fn reviews_fixture() -> Option<PathBuf> {
    cockpit_env("COCKPIT_REVIEWS_FIXTURE", "CLAUDE_REVIEWS_FIXTURE")
        .ok()
        .map(PathBuf::from)
}
pub fn claude_status_url() -> String {
    env::var("CLAUDE_STATUS_URL")
        .unwrap_or_else(|_| "https://status.claude.com/api/v2/summary.json".into())
}
pub fn codex_status_url() -> String {
    env::var("CODEX_STATUS_URL")
        .unwrap_or_else(|_| "https://status.openai.com/api/v2/summary.json".into())
}
