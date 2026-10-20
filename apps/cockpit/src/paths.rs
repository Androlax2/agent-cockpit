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

/// CLAUDE_SESSIONS_STATE_HOME first: the opencode launcher moves XDG_STATE_HOME into the sandbox's
/// private /tmp for opencode's own state, and every tool opencode runs inherits it; this one keeps
/// the cockpit's tools (claude-todo's list) on the real folder.
pub fn state_home() -> PathBuf {
    env::var("CLAUDE_SESSIONS_STATE_HOME").or_else(|_| env::var("XDG_STATE_HOME")).map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/state"))
}

fn from_env(name: &str, default: PathBuf) -> PathBuf {
    env::var(name).map(PathBuf::from).unwrap_or(default)
}

pub fn state_dir() -> PathBuf { state_home().join("claude-sessions") }
pub fn snapshot() -> PathBuf { runtime_dir().join("claude-sessions.json") }
pub fn current() -> PathBuf { runtime_dir().join("claude-sessions.current") }
/// While it exists, the phone page notifies: the lock screen's claude-sessions away on.
pub fn phone_away() -> PathBuf { runtime_dir().join("claude-remote-away") }
pub fn usage_cache() -> PathBuf { runtime_dir().join("claude-usage.json") }
pub fn codex_usage_cache() -> PathBuf { runtime_dir().join("codex-usage.json") }
/// Sessions' reports and requests: the one FIFO the sandboxes can write (greywall allowWrite).
pub fn osd_fifo() -> PathBuf { runtime_dir().join("osd.fifo") }
/// Everything else the OSD daemon takes (banners, focus, clearing, phases, volume): only programs
/// outside the sandbox reach this one.
pub fn osd_host_fifo() -> PathBuf { runtime_dir().join("osd-host.fifo") }
/// Session reports: a socket (osd-report.socket keeps it across daemon restarts), so the daemon
/// learns which process sent each report and takes the pane from where it really runs.
/// In a folder of its own, which the sandboxes mount whole: a socket file mounted alone stays the
/// old one in every running sandbox once the socket is made again.
pub fn osd_report_socket() -> PathBuf { runtime_dir().join("claude-report/osd-report.sock") }
/// The Claude sessions' scopes game-performance froze for a game, one per line; gone after it.
pub fn game_frozen() -> PathBuf { runtime_dir().join("game-performance.frozen") }
pub fn history() -> PathBuf { from_env("CLAUDE_SESSIONS_HISTORY", home().join(".claude/history.jsonl")) }
pub fn transcripts() -> PathBuf { from_env("CLAUDE_SESSIONS_TRANSCRIPTS", home().join(".claude/projects")) }
/// opencode's sessions and messages ($XDG_DATA_HOME/opencode/opencode.db), which its sandboxes write.
pub fn opencode_database() -> PathBuf {
    let data_home = env::var("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".local/share"));
    from_env("CLAUDE_SESSIONS_OPENCODE_DB", data_home.join("opencode/opencode.db"))
}
pub fn statusline_dir() -> PathBuf { home().join(".cache/claude") }
pub fn usage_script() -> PathBuf { home().join(".config/waybar/scripts/claude-usage.sh") }
pub fn credentials() -> PathBuf { home().join(".claude/.credentials.json") }
pub fn view_state() -> PathBuf { state_dir().join("view.json") }
pub fn continuity() -> PathBuf { state_dir().join("continuity.json") }
pub fn usage_archive() -> PathBuf { state_dir().join("usage.sqlite") }
pub fn alerts() -> PathBuf { state_dir().join("alerts.json") }
pub fn log() -> PathBuf { state_dir().join("log") }
pub fn todo() -> PathBuf { state_dir().join("todo.json") }
pub fn team_database() -> PathBuf { crate::audit::dir().join("teams.sqlite") }
pub fn team_agent_socket() -> PathBuf { runtime_dir().join("claude-team-host/agent.sock") }
pub fn settings() -> PathBuf { state_dir().join("settings.json") }
/// claude-todo's changes, queued by the OSD daemon with the sender's verified folder: the runtime
/// directory, which no sandbox sees (security audit 2026-10-19, M4).
pub fn todo_updates() -> PathBuf { from_env("CLAUDE_TODO_UPDATES", runtime_dir().join("claude-todo-updates.jsonl")) }
pub fn error_reports() -> PathBuf { state_dir().join("error-reports.json") }
pub fn error_report_dir() -> PathBuf { state_dir().join("error-reports") }
pub fn recent_sessions() -> PathBuf { state_dir().join("recent-sessions.json") }
pub fn ended_sessions() -> PathBuf { state_dir().join("ended-sessions") }
pub fn clones_dir() -> PathBuf { from_env("CLAUDE_SESSIONS_CLONES", home().join("IdeaProjects/.parallel")) }
pub fn greyproxy_db() -> PathBuf { from_env("CLAUDE_SESSIONS_GREYPROXY_DB", home().join(".local/share/greyproxy/greyproxy.db")) }
pub fn designs() -> PathBuf { from_env("CLAUDE_DESIGNS_DIR", home().join(".cache/claude/designs")) }
pub fn dotfiles() -> PathBuf { home().join("dotfiles") }
/// Where the dotfiles session works: a clone the host never runs or builds from. ~/dotfiles, the
/// stowed checkout, only moves forward through `claude-sessions apply-dotfiles`, after Theo read the
/// diff (security audit 2026-10-19, C1).
pub fn dotfiles_work() -> PathBuf { from_env("CLAUDE_DOTFILES_WORK", home().join("IdeaProjects/dotfiles")) }
pub fn reviews_fixture() -> Option<PathBuf> { env::var("CLAUDE_REVIEWS_FIXTURE").ok().map(PathBuf::from) }
pub fn claude_status_url() -> String {
    env::var("CLAUDE_STATUS_URL").unwrap_or_else(|_| "https://status.claude.com/api/v2/summary.json".into())
}
pub fn codex_status_url() -> String {
    env::var("CODEX_STATUS_URL").unwrap_or_else(|_| "https://status.openai.com/api/v2/summary.json".into())
}
