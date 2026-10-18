//! Small helpers shared by every part of the column: the shared log, the manager command, project
//! names, git branches, ages and money.

use crate::paths;
use chrono::{DateTime, Local, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub const LOG_MAX_BYTES: u64 = 1_048_576;
pub const LOG_LINE_MAX: usize = 300;

pub fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn now_secs() -> i64 {
    now() as i64
}

/// Whitespace runs as single spaces, trimmed.
pub fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A line in the shared log (format and rotation as claude-sessions' log_event). Never fails:
/// logging must not take the column down, and the state dir is read-only inside greywall.
pub fn log_event(level: &str, message: &str) {
    log_as("sidebar", level, message);
}

/// A line in the shared log under a component's name (the daemon logs as "daemon").
pub fn log_as(component: &str, level: &str, message: &str) {
    let path = paths::log();
    let _ = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        if fs::metadata(&path).map(|m| m.len() > LOG_MAX_BYTES).unwrap_or(false) {
            let mut rotated = path.clone().into_os_string();
            rotated.push(".1");
            fs::rename(&path, rotated)?;
        }
        let line: String = collapse(message).chars().take(LOG_LINE_MAX).collect();
        let stamp = Local::now().format("%Y-%m-%dT%H:%M:%S%:z");
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{stamp} {level} {component}: {line}")
    })();
}

/// Runs the manager command; its last error line, empty on success.
pub fn manager(arguments: &[&str]) -> String {
    let output = Command::new("claude-sessions").args(arguments).stdin(Stdio::null()).output();
    match output {
        Ok(output) if output.status.success() => String::new(),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last = stderr.trim().lines().last().unwrap_or("").to_string();
            if !last.is_empty() {
                log_event("ERROR", &format!("claude-sessions {}: {last}", arguments[0]));
                last
            } else {
                format!("claude-sessions {} failed", arguments[0])
            }
        }
        Err(error) => format!("claude-sessions {}: {error}", arguments[0]),
    }
}

/// For exit and restart, which wait on Claude for seconds: the column must not freeze meanwhile.
/// Nobody sees its terminal, so the manager sends its error messages to the log.
pub fn manager_in_background(arguments: &[&str]) {
    let _ = Command::new("claude-sessions")
        .args(arguments)
        .env("CLAUDE_SESSIONS_LOG_STDERR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
}

/// A command's stdout when it succeeds.
pub fn output_of(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Written to a new file beside it, then renamed over it. Several of these folders are the
/// sandbox's to write (runs, guides): a symlink planted at the path or at a predictable temporary
/// name must not make Theo's programs write through it, so the temporary file has a random name
/// and is created, never opened (create_new does not follow symlinks), and the rename replaces a
/// link at the path instead of writing to its target.
pub fn write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".tmp-{}", token_hex(6)));
    let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temporary)?;
    file.write_all(value.to_string().as_bytes())?;
    drop(file);
    fs::rename(&temporary, path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })
}

pub fn current_session() -> String {
    fs::read_to_string(paths::current()).map(|text| text.trim().to_string()).unwrap_or_default()
}

/// The git checkout holding `path` (Claude often works in a subdirectory), or `path` itself.
pub fn repository_root(path: &str) -> String {
    let mut directory = PathBuf::from(path);
    loop {
        if directory.as_os_str().is_empty() || directory == Path::new("/") {
            return path.to_string();
        }
        if directory.join(".git").exists() {
            return directory.to_string_lossy().into_owned();
        }
        if !directory.pop() {
            return path.to_string();
        }
    }
}

/// A directory's project: the repository for a claude-sessions clone (<clones>/<repo>/<name>),
/// else the directory's own name. Same rule as the daemon's registry::parallel_clone_repository.
pub fn project_name(path: &str) -> String {
    if !path.is_empty() {
        if let Ok(relative) = Path::new(path).strip_prefix(paths::clones_dir()) {
            let parts: Vec<_> = relative.components().collect();
            if parts.len() >= 2 {
                return parts[0].as_os_str().to_string_lossy().into_owned();
            }
        }
    }
    let trimmed = path.trim_end_matches('/');
    match Path::new(trimmed).file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => path.to_string(),
    }
}

/// The HEAD file of the repository at cwd, through a worktree's gitdir file.
fn git_head_path(cwd: &str) -> Option<PathBuf> {
    let dot_git = Path::new(cwd).join(".git");
    if dot_git.is_dir() {
        return Some(dot_git.join("HEAD"));
    }
    let first = fs::read_to_string(&dot_git).ok()?;
    let gitdir = first.lines().next()?.trim().strip_prefix("gitdir:")?.trim().to_string();
    let gitdir = Path::new(&gitdir);
    Some(if gitdir.is_absolute() { gitdir.join("HEAD") } else { Path::new(cwd).join(gitdir).join("HEAD") })
}

static BRANCHES: Mutex<Option<HashMap<String, (SystemTime, String)>>> = Mutex::new(None);

/// The checked-out branch (or a short commit when detached), read from HEAD itself: no git
/// process per tick. Cached by HEAD's mtime.
pub fn git_branch(cwd: &str) -> String {
    if cwd.is_empty() {
        return String::new();
    }
    let Some(head) = git_head_path(cwd) else { return String::new() };
    let Ok(mtime) = fs::metadata(&head).and_then(|m| m.modified()) else { return String::new() };
    let mut cache = BRANCHES.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((cached_mtime, branch)) = cache.get(cwd) {
        if *cached_mtime == mtime {
            return branch.clone();
        }
    }
    let Ok(content) = fs::read_to_string(&head) else { return String::new() };
    let content = content.trim();
    let branch = match content.strip_prefix("ref: refs/heads/") {
        Some(branch) => branch.to_string(),
        None => content.chars().take(7).collect(),
    };
    cache.insert(cwd.to_string(), (mtime, branch.clone()));
    branch
}

pub fn age(since: i64) -> String {
    let seconds = now_secs() - since;
    if seconds < 60 {
        format!("{seconds}s")
    } else if seconds < 3600 {
        format!("{}m", seconds / 60)
    } else if seconds < 86400 {
        format!("{}h", seconds / 3600)
    } else {
        format!("{}d", seconds / 86400)
    }
}

/// "$0.42", "$316", "$2,780", "$12.3k".
pub fn money(amount: f64) -> String {
    if amount >= 10000.0 {
        return format!("${:.1}k", amount / 1000.0);
    }
    if amount >= 100.0 {
        let whole = amount.round() as i64;
        let digits = whole.to_string();
        let mut grouped = String::new();
        for (index, digit) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(digit);
        }
        return format!("${grouped}");
    }
    format!("${amount:.2}")
}

pub fn parse_time(stamp: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(stamp).ok().map(|moment| moment.with_timezone(&Utc))
}

/// 4 random bytes as 8 hex characters, for todo ids and report names.
pub fn token_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    if let Ok(mut random) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = random.read_exact(&mut buffer);
    }
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// hyprctl finds Hyprland through HYPRLAND_INSTANCE_SIGNATURE. Processes that outlive a Hyprland
/// restart, or start outside it, hold a stale or no value: take the running instance, the newest
/// $XDG_RUNTIME_DIR/hypr/<id> with a socket.
pub fn refresh_hyprland_instance() {
    let base = paths::runtime_dir().join("hypr");
    if let Ok(current) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") {
        if !current.is_empty() && base.join(&current).join(".socket.sock").exists() {
            return;
        }
    }
    let Ok(entries) = fs::read_dir(&base) else { return };
    let newest = entries
        .flatten()
        .filter(|entry| entry.path().join(".socket.sock").exists())
        .max_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok());
    if let Some(entry) = newest {
        // SAFETY: the column is single-threaded when it refreshes this (before spawning hyprctl).
        unsafe { std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", entry.file_name()) };
    }
}

/// Game mode (the gamemode submap) or a game window focused; same rule as the daemon's.
pub fn gaming() -> bool {
    let game_class = regex::Regex::new(r"^(steam_app_[0-9]+|Star Citizen|Dofus\.x64|retroarch)$").unwrap();
    let submap = output_of("hyprctl", &["submap"]).unwrap_or_default();
    let window = output_of("hyprctl", &["-j", "activewindow"]).unwrap_or_default();
    let class = serde_json::from_str::<Value>(&window)
        .ok()
        .and_then(|window| window.get("class").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default();
    submap.trim() == "gamemode" || game_class.is_match(&class)
}

/// One banner through the OSD daemon's FIFO; false when the daemon is not reading it.
pub fn send_notice(notice: &Value) -> bool {
    use std::os::unix::fs::OpenOptionsExt;
    let Ok(mut fifo) = OpenOptions::new().write(true).custom_flags(libc_nonblock()).open(paths::osd_fifo()) else {
        return false;
    };
    fifo.write_all(format!("notice {notice}\n").as_bytes()).is_ok()
}

const fn libc_nonblock() -> i32 {
    0o4000 // O_NONBLOCK on Linux
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_matches_the_python_column() {
        assert_eq!(money(0.4213), "$0.42");
        assert_eq!(money(316.4), "$316");
        assert_eq!(money(2780.0), "$2,780");
        assert_eq!(money(12345.0), "$12.3k");
    }

    #[test]
    fn collapse_joins_whitespace() {
        assert_eq!(collapse("  add  a\n healthcheck "), "add a healthcheck");
    }
}
