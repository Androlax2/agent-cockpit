//! File notifications and full-refresh scheduling for the sidebar.

use std::collections::HashMap;
use std::ffi::{CString, OsStr};
use std::fs;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::sync::atomic::{AtomicBool, Ordering};

static BACKGROUND_CHANGED: AtomicBool = AtomicBool::new(false);

/// Called explicitly after publishing a worker result; the terminal consumes it without cloning
/// provider summaries or review lists on each input wait.
pub fn background_changed() { BACKGROUND_CHANGED.store(true, Ordering::Release); }
pub fn take_background_changed() -> bool { BACKGROUND_CHANGED.swap(false, Ordering::Acquire) }

pub const SAFETY_INTERVAL: Duration = Duration::from_secs(5);
const CHANGE_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
pub struct Changes {
    pub refresh: bool,
    pub immediate: bool,
}

#[derive(Clone)]
pub struct Target {
    path: PathBuf,
    tree: bool,
    immediate: bool,
    statuslines: bool,
}

impl Target {
    pub fn file(path: PathBuf, immediate: bool) -> Self { Self { path, tree: false, immediate, statuslines: false } }
    pub fn tree(path: PathBuf) -> Self { Self { path, tree: true, immediate: false, statuslines: false } }
    pub fn statuslines(path: PathBuf) -> Self { Self { path, tree: false, immediate: false, statuslines: true } }

    fn matches(&self, path: &Path) -> bool {
        path == self.path || self.path.starts_with(path) || (self.tree && path.starts_with(&self.path))
            || (self.statuslines && path.parent() == Some(self.path.as_path()) && path.file_name().is_some_and(|name| {
                let name = name.as_bytes();
                name.starts_with(b"statusline-") && name.ends_with(b".json")
            }))
    }
}

pub fn inputs() -> Vec<Target> {
    use crate::{paths, remote};
    let mut targets = vec![Target::file(paths::current(), true), Target::statuslines(paths::statusline_dir())];
    for path in [paths::snapshot(), paths::usage_cache(), paths::codex_usage_cache(), paths::todo_updates(),
        paths::game_frozen(), paths::loading_state(), paths::todo(), paths::settings(), paths::view_state(),
        paths::recent_sessions(), paths::history(), paths::credentials(), remote::state_dir().join("devices.json"),
        paths::runtime_dir().join("claude-usage-error.json"), paths::runtime_dir().join("codex-usage-error.json")] {
        targets.push(Target::file(path, false));
    }
    for path in [paths::designs(), paths::restarting_dir(), paths::asleep_dir(),
        paths::ended_sessions(), paths::state_dir().join("prompt-queue"), paths::state_dir().join("restart-when-idle")] {
        targets.push(Target::tree(path));
    }
    targets
}

pub struct Watcher {
    descriptor: OwnedFd,
    targets: Vec<Target>,
    directories: HashMap<i32, PathBuf>,
}

impl Watcher {
    pub fn new(targets: Vec<Target>) -> io::Result<Self> {
        // SAFETY: inotify_init1 takes flags only and returns an owned descriptor on success.
        let descriptor = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if descriptor < 0 { return Err(io::Error::last_os_error()); }
        let mut watcher = Self {
            // SAFETY: this descriptor was just created and has exactly one owner.
            descriptor: unsafe { OwnedFd::from_raw_fd(descriptor) }, targets, directories: HashMap::new(),
        };
        for target in watcher.targets.clone() {
            let mut parent = target.path.parent().unwrap_or(Path::new("/"));
            while !parent.is_dir() {
                parent = parent.parent().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no watchable ancestor"))?;
            }
            watcher.add_directory(parent)?;
            if target.tree && target.path.is_dir() { watcher.add_tree(&target.path)?; }
            if target.statuslines && target.path.is_dir() { watcher.add_directory(&target.path)?; }
        }
        Ok(watcher)
    }

    fn add_directory(&mut self, path: &Path) -> io::Result<()> {
        if self.directories.values().any(|watched| watched == path) { return Ok(()); }
        let name = CString::new(path.as_os_str().as_bytes())?;
        let mask = libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_MOVED_FROM | libc::IN_CREATE
            | libc::IN_DELETE | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF | libc::IN_ONLYDIR;
        // SAFETY: name is NUL-terminated and descriptor is owned and open.
        let watch = unsafe { libc::inotify_add_watch(self.descriptor.as_raw_fd(), name.as_ptr(), mask) };
        if watch < 0 { return Err(io::Error::last_os_error()); }
        self.directories.insert(watch, path.to_owned());
        Ok(())
    }

    fn add_tree(&mut self, path: &Path) -> io::Result<()> {
        self.add_directory(path)?;
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            // Do not follow links out of the selected input tree.
            if entry.file_type()?.is_dir() { self.add_tree(&entry.path())?; }
        }
        Ok(())
    }

    pub fn repair(&mut self) -> io::Result<()> {
        // Install replacement watches before closing the old descriptor. Atomic-renamed input
        // files remain watched through their directory, not through their previous inode.
        *self = Self::new(self.targets.clone())?;
        Ok(())
    }

    pub fn changes(&mut self) -> io::Result<Changes> {
        let mut changes = Changes::default();
        let mut repair = false;
        let mut buffer = [0_u8; 16384];
        for _ in 0..16 {
            // SAFETY: buffer is writable for its full length; read does not retain it.
            let count = unsafe { libc::read(self.descriptor.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock { break; }
                if error.kind() == io::ErrorKind::Interrupted { continue; }
                return Err(error);
            }
            if count == 0 { return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "inotify closed")); }
            let mut offset = 0;
            while offset + size_of::<libc::inotify_event>() <= count as usize {
                // SAFETY: the header fits in this read; byte buffers need an unaligned load.
                let event = unsafe { buffer.as_ptr().add(offset).cast::<libc::inotify_event>().read_unaligned() };
                let start = offset + size_of::<libc::inotify_event>();
                let end = start + event.len as usize;
                if end > count as usize { return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated inotify event")); }
                if event.mask & libc::IN_Q_OVERFLOW != 0 {
                    return Err(io::Error::other("input watch overflowed"));
                }
                if event.mask & (libc::IN_IGNORED | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) != 0 {
                    changes.refresh = true;
                    repair = true;
                }
                if let Some(directory) = self.directories.get(&event.wd) {
                    let name = &buffer[start..end];
                    let length = name.iter().position(|byte| *byte == 0).unwrap_or(name.len());
                    let path = directory.join(OsStr::from_bytes(&name[..length]));
                    for target in &self.targets {
                        if target.matches(&path) {
                            changes.refresh = true;
                            changes.immediate |= target.immediate;
                            repair |= event.mask & libc::IN_ISDIR != 0;
                        }
                    }
                }
                offset = end;
            }
        }
        if repair { self.repair()?; }
        Ok(changes)
    }
}

#[derive(Default)]
pub struct Schedule {
    last_full: Option<Duration>,
    last_maintenance: Option<Duration>,
    dirty: bool,
}

impl Schedule {
    /// Expensive work earns its cooldown after completion, not before it starts. Otherwise a
    /// slow gather plus frequent reports can keep the terminal busy with back-to-back gathers.
    pub fn finished(&mut self, now: Duration) { self.last_full = Some(now); }

    pub fn maintenance_due(&mut self, now: Duration) -> bool {
        if self.last_maintenance.is_none_or(|last| now.saturating_sub(last) >= Duration::from_secs(1)) {
            self.last_maintenance = Some(now);
            return true;
        }
        false
    }

    pub fn due(&mut self, now: Duration, changes: Changes, healthy: bool) -> bool {
        self.dirty |= changes.refresh;
        let elapsed = self.last_full.map(|last| now.saturating_sub(last));
        let interval = if healthy { SAFETY_INTERVAL } else { CHANGE_INTERVAL };
        if elapsed.is_none_or(|elapsed| elapsed >= interval)
            || changes.immediate || (self.dirty && elapsed.is_some_and(|elapsed| elapsed >= CHANGE_INTERVAL)) {
            self.last_full = Some(now);
            self.dirty = false;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, time::Duration};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("sidebar-watch-{}", crate::util::token_hex(6)));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) { fs::remove_dir_all(&self.0).unwrap(); }
    }

    #[test]
    fn file_changes_survive_atomic_replacement_and_deletion() {
        let fixture = Fixture::new();
        let path = fixture.0.join("current");
        let mut watcher = Watcher::new(vec![Target::file(path.clone(), true)]).unwrap();
        for content in ["a", "b"] {
            fs::write(fixture.0.join("temporary"), content).unwrap();
            fs::rename(fixture.0.join("temporary"), &path).unwrap();
            assert!(watcher.changes().unwrap().immediate);
        }
        fs::remove_file(&path).unwrap();
        assert!(watcher.changes().unwrap().immediate);
        fs::write(&path, "c").unwrap();
        assert!(watcher.changes().unwrap().immediate);
    }

    #[test]
    fn reading_inputs_and_writing_unrelated_files_do_not_request_a_refresh() {
        let fixture = Fixture::new();
        let path = fixture.0.join("snapshot");
        fs::write(&path, "{}").unwrap();
        let mut watcher = Watcher::new(vec![Target::file(path.clone(), false)]).unwrap();
        fs::read(&path).unwrap();
        fs::write(fixture.0.join("log"), "unrelated").unwrap();
        assert!(!watcher.changes().unwrap().refresh);
        fs::write(path, "updated").unwrap();
        assert!(watcher.changes().unwrap().refresh);
        assert!(!watcher.changes().unwrap().refresh);
    }

    #[test]
    fn a_missing_tree_and_its_replacement_are_watched_after_repair() {
        let fixture = Fixture::new();
        let directory = fixture.0.join("queue");
        let mut watcher = Watcher::new(vec![Target::tree(directory.clone())]).unwrap();
        fs::create_dir_all(directory.join("session")).unwrap();
        assert!(watcher.changes().unwrap().refresh);
        watcher.repair().unwrap();
        fs::write(directory.join("session/1.txt"), "first").unwrap();
        assert!(watcher.changes().unwrap().refresh);
        fs::remove_dir_all(&directory).unwrap();
        fs::create_dir_all(directory.join("session")).unwrap();
        assert!(watcher.changes().unwrap().refresh);
        watcher.repair().unwrap();
        fs::write(directory.join("session/2.txt"), "second").unwrap();
        assert!(watcher.changes().unwrap().refresh);
    }

    #[test]
    fn idle_waits_for_the_safety_refresh_but_input_changes_are_prompt() {
        let mut schedule = Schedule::default();
        assert!(schedule.due(Duration::ZERO, Changes::default(), true));
        assert!(!schedule.due(Duration::from_millis(250), Changes::default(), true));
        assert!(!schedule.due(Duration::from_secs(4), Changes::default(), true));
        assert!(schedule.due(Duration::from_secs(5), Changes::default(), true));
        assert!(schedule.due(Duration::from_millis(5040), Changes { refresh: true, immediate: true }, true));
    }

    #[test]
    fn bursts_coalesce_without_losing_the_last_change() {
        let mut schedule = Schedule::default();
        schedule.due(Duration::ZERO, Changes::default(), true);
        assert!(!schedule.due(Duration::from_millis(40), Changes { refresh: true, immediate: false }, true));
        assert!(!schedule.due(Duration::from_millis(200), Changes { refresh: true, immediate: false }, true));
        assert!(schedule.due(Duration::from_millis(250), Changes::default(), true));
        assert!(!schedule.due(Duration::from_millis(500), Changes::default(), true));
    }

    #[test]
    fn broken_watch_coverage_uses_the_previous_polling_interval() {
        let mut schedule = Schedule::default();
        schedule.due(Duration::ZERO, Changes::default(), false);
        assert!(!schedule.due(Duration::from_millis(200), Changes::default(), false));
        assert!(schedule.due(Duration::from_millis(250), Changes::default(), false));
    }

    #[test]
    fn maintenance_keeps_running_without_any_input_changes() {
        let mut schedule = Schedule::default();
        assert!(schedule.maintenance_due(Duration::ZERO));
        assert!(!schedule.maintenance_due(Duration::from_millis(40)));
        assert!(schedule.maintenance_due(Duration::from_secs(1)));
        assert!(!schedule.maintenance_due(Duration::from_millis(1040)));
        assert!(schedule.maintenance_due(Duration::from_secs(2)));
    }

    #[test]
    fn a_slow_refresh_gets_a_cooldown_after_it_finishes() {
        let mut schedule = Schedule::default();
        assert!(schedule.due(Duration::ZERO, Changes::default(), true));
        schedule.finished(Duration::from_millis(400));
        assert!(!schedule.due(Duration::from_millis(440), Changes { refresh: true, immediate: false }, true));
        assert!(schedule.due(Duration::from_millis(650), Changes::default(), true));
    }

    #[test]
    fn statusline_watches_ignore_hook_logs_and_request_subtrees() {
        let fixture = Fixture::new();
        let mut watcher = Watcher::new(vec![Target::statuslines(fixture.0.clone())]).unwrap();
        fs::write(fixture.0.join("hook.log"), "noise").unwrap();
        fs::create_dir(fixture.0.join("requests")).unwrap();
        fs::write(fixture.0.join("requests/request.json"), "{}").unwrap();
        assert!(!watcher.changes().unwrap().refresh);
        fs::write(fixture.0.join("statusline-session.json"), "{}").unwrap();
        assert!(watcher.changes().unwrap().refresh);
    }
}
