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

/// A line in the shared log (format and rotation as cockpit' log_event). Never fails:
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
        // No control character reaches the log: a session's text in it could carry escape codes to
        // the terminal `cockpit logs` prints to (security audit 2026-10-19, M6).
        let line: String = collapse(message).chars().filter(|c| !c.is_control()).take(LOG_LINE_MAX).collect();
        let stamp = Local::now().format("%Y-%m-%dT%H:%M:%S%:z");
        let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
        writeln!(file, "{stamp} {level} {component}: {line}")
    })();
}

/// Runs the manager command; its last error line, empty on success.
pub fn manager(arguments: &[&str]) -> String {
    let output = Command::new("cockpit").args(arguments).stdin(Stdio::null()).output();
    match output {
        Ok(output) if output.status.success() => String::new(),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last = stderr.trim().lines().last().unwrap_or("").to_string();
            if !last.is_empty() {
                log_event("ERROR", &format!("cockpit {}: {last}", arguments[0]));
                last
            } else {
                format!("cockpit {} failed", arguments[0])
            }
        }
        Err(error) => format!("cockpit {}: {error}", arguments[0]),
    }
}

/// For exit and restart, which wait on Claude for seconds: the column must not freeze meanwhile.
/// Nobody sees its terminal, so the manager sends its error messages to the log.
pub fn manager_in_background(arguments: &[&str]) {
    let _ = manager_child(arguments);
}

/// The manager running in the background, for a caller that wants to know how it ended.
pub fn manager_child(arguments: &[&str]) -> std::io::Result<std::process::Child> {
    Command::new("cockpit")
        .args(arguments)
        .env("COCKPIT_LOG_STDERR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}

/// A command's stdout when it succeeds.
pub fn output_of(program: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new(program).args(arguments).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// A file a sandbox can write (its project, ~/.claude, ~/.cache/claude), opened without following a
/// final symlink and without blocking on a FIFO planted there, and only when it is a regular file:
/// a FIFO, /dev/zero or a socket must not freeze the column or the daemon (security audit
/// 2026-10-19, M5).
pub fn open_untrusted(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    const O_NOFOLLOW: i32 = 0o400000;
    const O_NONBLOCK: i32 = 0o4000;
    let file = fs::OpenOptions::new().read(true).custom_flags(O_NOFOLLOW | O_NONBLOCK).open(path)?;
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other(format!("{} is not a regular file", path.display())));
    }
    Ok(file)
}

/// At most `limit` bytes of a file a sandbox can write (open_untrusted): a huge file costs no more.
pub fn read_untrusted(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    open_untrusted(path)?.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// read_untrusted as text, invalid UTF-8 replaced.
pub fn read_untrusted_text(path: &Path, limit: u64) -> std::io::Result<String> {
    read_untrusted(path, limit).map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
}

/// A small JSON file a sandbox can write; None when it is not one (or past 1 MiB).
pub fn read_untrusted_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&read_untrusted(path, 1 << 20).ok()?).ok()
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

/// apps/claude.lua (WORKSPACE) and waybar/scripts/on-claude-workspace.sh carry the same number.
pub const CLAUDE_WORKSPACE: i64 = 6;

/// Whether Theo is on the Claude workspace right now.
pub fn claude_workspace_active() -> bool {
    output_of("hyprctl", &["-j", "activeworkspace"]).and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .is_some_and(|workspace| workspace.get("id").and_then(Value::as_i64) == Some(CLAUDE_WORKSPACE))
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

/// A directory's project: the repository for a cockpit clone (<clones>/<repo>/<name>),
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

/// Where a session for an item filed in `directory` starts. A finished clone is deleted and the
/// manager refuses a folder that is gone, so an item filed in one starts from its repository's
/// own checkout (~/IdeaProjects/<repository>, the projects the picker lists).
pub fn start_directory(directory: &str) -> Result<String, String> {
    let path = Path::new(directory);
    if path.is_dir() || !path.starts_with(paths::clones_dir()) {
        return Ok(directory.to_string());
    }
    let checkout = paths::home().join("IdeaProjects").join(project_name(directory));
    if !checkout.is_dir() {
        return Err(format!("its clone is gone and there is no {} to start from", checkout.display()));
    }
    Ok(checkout.to_string_lossy().into_owned())
}

/// A .git file or a HEAD is a line: anything longer is not one.
const GIT_FILE_MAX: u64 = 4096;

/// The HEAD file of the repository at cwd, through a worktree's gitdir file.
fn git_head_path(cwd: &str) -> Option<PathBuf> {
    let dot_git = Path::new(cwd).join(".git");
    if dot_git.is_dir() {
        return Some(dot_git.join("HEAD"));
    }
    let first = read_untrusted_text(&dot_git, GIT_FILE_MAX).ok()?;
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
    let Ok(content) = read_untrusted_text(&head, GIT_FILE_MAX) else { return String::new() };
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
    if let Some(instance) = running_hyprland_instance() {
        // SAFETY: the column is single-threaded when it refreshes this (before spawning hyprctl).
        unsafe { std::env::set_var("HYPRLAND_INSTANCE_SIGNATURE", instance) };
    }
}

/// The signature hyprctl needs: this process's own when its Hyprland still runs, else the newest
/// $XDG_RUNTIME_DIR/hypr/<id> with a socket; None when no Hyprland runs.
pub fn running_hyprland_instance() -> Option<std::ffi::OsString> {
    let base = paths::runtime_dir().join("hypr");
    if let Some(current) = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE") {
        if !current.is_empty() && base.join(&current).join(".socket.sock").exists() {
            return Some(current);
        }
    }
    fs::read_dir(&base).ok()?
        .flatten()
        .filter(|entry| entry.path().join(".socket.sock").exists())
        .max_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok())
        .map(|entry| entry.file_name())
}

/// Game mode (the gamemode submap) or a game window focused; same rule as the daemon's.
pub fn gaming() -> bool {
    let submap = output_of("hyprctl", &["submap"]).unwrap_or_default();
    submap.trim() == "gamemode" || active_game().is_some()
}

/// The focused window's title (its class when untitled) when that window is a game.
pub fn active_game() -> Option<String> {
    static GAME_CLASS: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^(steam_app_[0-9]+|Star Citizen|Dofus\.x64|retroarch)$").unwrap());
    let game_class = &*GAME_CLASS;
    let window: Value = serde_json::from_str(&output_of("hyprctl", &["-j", "activewindow"])?).ok()?;
    let class = window.get("class").and_then(Value::as_str)?;
    let title = window.get("title").and_then(Value::as_str).map(str::trim).filter(|title| !title.is_empty());
    game_class.is_match(class).then(|| title.unwrap_or(class).to_string())
}

/// The fullest of paths::disk_mounts as (mount, percent used); the first wins a tie, so a single
/// filesystem reads as /. None when none could be measured.
pub fn fullest_disk() -> Option<(String, f64)> {
    paths::disk_mounts().into_iter()
        .filter_map(|mount| disk_used_percent(&mount).map(|used| (mount, used)))
        .fold(None, |fullest, (mount, used)| {
            if fullest.as_ref().is_some_and(|(_, top)| *top >= used) { fullest } else { Some((mount, used)) }
        })
}

fn disk_used_percent(mount: &str) -> Option<f64> {
    let path = std::ffi::CString::new(mount).ok()?;
    // SAFETY: statvfs fills the zeroed struct and reads only the NUL-terminated path.
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    used_percent(stats.f_blocks as u64, stats.f_bfree as u64, stats.f_bavail as u64)
}

/// df's Use%: the blocks reserved for root count as neither used nor available.
pub fn used_percent(blocks: u64, free: u64, available: u64) -> Option<f64> {
    let used = blocks.checked_sub(free)?;
    let total = used + available;
    (total > 0).then(|| (used as f64 * 100.0 / total as f64).ceil())
}

/// One banner through the OSD daemon's FIFO; false when the daemon is not reading it.
pub fn send_notice(notice: &Value) -> bool {
    use std::os::unix::fs::OpenOptionsExt;
    let Ok(mut fifo) = OpenOptions::new().write(true).custom_flags(libc_nonblock()).open(paths::osd_host_fifo()) else {
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
    fn used_percent_matches_df_by_leaving_roots_reserve_out() {
        assert_eq!(used_percent(1000, 150, 100), Some(90.0));
        assert_eq!(used_percent(1000, 1000, 950), Some(0.0));
        assert_eq!(used_percent(0, 0, 0), None);
    }

    #[test]
    fn collapse_joins_whitespace() {
        assert_eq!(collapse("  add  a\n healthcheck "), "add a healthcheck");
    }
}
