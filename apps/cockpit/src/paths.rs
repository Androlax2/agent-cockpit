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

pub fn state_home() -> PathBuf {
    env::var("XDG_STATE_HOME").map(PathBuf::from).unwrap_or_else(|_| home().join(".local/state"))
}

fn from_env(name: &str, default: PathBuf) -> PathBuf {
    env::var(name).map(PathBuf::from).unwrap_or(default)
}

pub fn state_dir() -> PathBuf { state_home().join("claude-sessions") }
pub fn snapshot() -> PathBuf { runtime_dir().join("claude-sessions.json") }
pub fn current() -> PathBuf { runtime_dir().join("claude-sessions.current") }
pub fn usage_cache() -> PathBuf { runtime_dir().join("claude-usage.json") }
/// Sessions' reports and requests: the one FIFO the sandboxes can write (greywall allowWrite).
pub fn osd_fifo() -> PathBuf { runtime_dir().join("osd.fifo") }
/// Everything else the OSD daemon takes (banners, focus, clearing, phases, volume): only programs
/// outside the sandbox reach this one.
pub fn osd_host_fifo() -> PathBuf { runtime_dir().join("osd-host.fifo") }
pub fn history() -> PathBuf { from_env("CLAUDE_SESSIONS_HISTORY", home().join(".claude/history.jsonl")) }
pub fn transcripts() -> PathBuf { from_env("CLAUDE_SESSIONS_TRANSCRIPTS", home().join(".claude/projects")) }
pub fn statusline_dir() -> PathBuf { home().join(".cache/claude") }
pub fn usage_script() -> PathBuf { home().join(".config/waybar/scripts/claude-usage.sh") }
pub fn credentials() -> PathBuf { home().join(".claude/.credentials.json") }
pub fn view_state() -> PathBuf { state_dir().join("view.json") }
pub fn usage_archive() -> PathBuf { state_dir().join("usage.sqlite") }
pub fn alerts() -> PathBuf { state_dir().join("alerts.json") }
pub fn log() -> PathBuf { state_dir().join("log") }
pub fn todo() -> PathBuf { state_dir().join("todo.json") }
pub fn todo_updates() -> PathBuf { from_env("CLAUDE_TODO_UPDATES", home().join(".cache/claude/todo/updates.jsonl")) }
pub fn error_reports() -> PathBuf { state_dir().join("error-reports.json") }
pub fn error_report_dir() -> PathBuf { state_dir().join("error-reports") }
pub fn recent_sessions() -> PathBuf { state_dir().join("recent-sessions.json") }
pub fn ended_sessions() -> PathBuf { state_dir().join("ended-sessions") }
pub fn clones_dir() -> PathBuf { from_env("CLAUDE_SESSIONS_CLONES", home().join("IdeaProjects/.parallel")) }
pub fn greyproxy_db() -> PathBuf { from_env("CLAUDE_SESSIONS_GREYPROXY_DB", home().join(".local/share/greyproxy/greyproxy.db")) }
pub fn designs() -> PathBuf { from_env("CLAUDE_DESIGNS_DIR", home().join(".cache/claude/designs")) }
pub fn dotfiles() -> PathBuf { home().join("dotfiles") }
pub fn reviews_fixture() -> Option<PathBuf> { env::var("CLAUDE_REVIEWS_FIXTURE").ok().map(PathBuf::from) }
pub fn claude_status_url() -> String {
    env::var("CLAUDE_STATUS_URL").unwrap_or_else(|_| "https://status.claude.com/api/v2/summary.json".into())
}
