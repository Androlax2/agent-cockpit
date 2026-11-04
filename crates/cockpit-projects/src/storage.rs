//! Bounded metadata-only observations of registered clone build folders and of the folders
//! directly inside one parent, such as the sessions' temporary folders.

use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use cockpit_turso::Database;
use rustix::fs::{self, AtFlags, Dir, FileType, Mode, OFlags};
use rustix::io::Errno;

use crate::store::{StoredCheckout, stored_clones_limited};
use crate::{Checkout, CheckoutState, Error, blocking};

const MAX_CLONES: usize = 128;
// Per clone, not per report: a real Rust `target/` holds hundreds of thousands of entries,
// and a shared allowance let the first clone starve every later one (#764).
const MAX_ENTRIES: usize = 500_000;
const MAX_FOLDERS: usize = 128;
const MAX_DEPTH: usize = 64;
// Upper bound for one clone; `measure_all` shrinks it so every clone still gets a turn.
const SCAN_BUDGET: Duration = Duration::from_secs(3);
const REPORT_BUDGET: Duration = Duration::from_secs(30);
const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
// Ancestors and the identity directory are anchors, never enumerated. Linux can open
// these without directory-read permission; `DIRECTORY` also rejects symlinks with O_PATH.
#[cfg(target_os = "linux")]
const ANCHOR_FLAGS: OFlags = OFlags::PATH
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
#[cfg(not(target_os = "linux"))]
const ANCHOR_FLAGS: OFlags = DIRECTORY_FLAGS;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetSize {
    Complete { apparent_bytes: u64 },
    Missing,
    Incomplete { apparent_bytes: u64, reason: String },
    Unavailable { reason: String },
}

#[derive(Debug, Clone)]
pub struct CloneStorage {
    /// Registration and occupancy as observed before the filesystem scan.
    pub checkout: Checkout,
    pub measurement: TargetSize,
}

#[derive(Debug, Clone)]
pub struct StorageReport {
    pub clones: Vec<CloneStorage>,
    pub truncated: bool,
}

/// Reports the literal `target/` of at most 128 registered parallel/review clones.
/// Each clone has its own entry and time allowance, so a huge `target/` yields an
/// incomplete count that says how far the walk got, never an empty row for its neighbours.
/// Filesystem work runs off the executor, without holding a database transaction. This is
/// not an atomic snapshot, allocated-space measurement, or indication that pruning is safe.
pub async fn clone_storage(database: &Database) -> Result<StorageReport, Error> {
    let mut stored = {
        let read = database.read().await?;
        stored_clones_limited(&read, (MAX_CLONES + 1) as i64).await?
    };
    let truncated = stored.len() > MAX_CLONES;
    stored.truncate(MAX_CLONES);
    blocking(move || {
        let clones = measure_all(stored, MAX_ENTRIES, SCAN_BUDGET, REPORT_BUDGET);
        Ok(StorageReport { clones, truncated })
    })
    .await
}

fn measure_all(
    stored: Vec<StoredCheckout>,
    entries_per_clone: usize,
    time_per_clone: Duration,
    report_time: Duration,
) -> Vec<CloneStorage> {
    let report_deadline = Instant::now() + report_time;
    let clone_count = stored.len();
    stored
        .into_iter()
        .enumerate()
        .map(|(index, stored)| {
            let clones_left = u32::try_from(clone_count - index).unwrap_or(u32::MAX);
            let fair_share = fair_time_share(report_deadline, clones_left);
            let mut budget = Budget::new(entries_per_clone, fair_share.min(time_per_clone));
            let measurement = measure(&stored, &mut budget, &mut || {});
            CloneStorage {
                checkout: stored.checkout,
                measurement,
            }
        })
        .collect()
}

/// Unused time flows to the clones still waiting, so a small `target/` early on gives its
/// neighbours more than an even split of the report budget.
fn fair_time_share(report_deadline: Instant, clones_left: u32) -> Duration {
    report_deadline.saturating_duration_since(Instant::now()) / clones_left
}

/// One entry directly inside the measured parent.
#[derive(Debug, Clone)]
pub struct FolderStorage {
    pub name: String,
    pub path: PathBuf,
    pub measurement: TargetSize,
}

#[derive(Debug, Clone, Default)]
pub struct FolderReport {
    /// In name order.
    pub folders: Vec<FolderStorage>,
    /// Further entries were omitted by the row or listing limit.
    pub truncated: bool,
    /// Why `parent` itself could not be listed; `folders` is then empty or partial.
    pub unavailable: Option<String>,
}

/// Measures each entry directly inside `parent`, at most 128 in name order, under the same
/// per-folder bounds as the clone report; an entry that is not a folder is listed as unavailable. An
/// absent `parent` holds none. Never follows a link, at `parent`'s last component included.
/// A `parent` that cannot be listed is reported, not an error: the caller's other
/// measurements stand.
pub async fn folder_storage(parent: PathBuf) -> FolderReport {
    blocking(move || Ok(measure_folders(&parent)))
        .await
        .unwrap_or_else(|error| FolderReport {
            unavailable: Some(error.to_string()),
            ..FolderReport::default()
        })
}

fn measure_folders(parent: &Path) -> FolderReport {
    let unavailable = |action: &str, error: Errno| FolderReport {
        unavailable: Some(format!("cannot {action} {}: {error}", parent.display())),
        ..FolderReport::default()
    };
    let directory = match fs::open(parent, DIRECTORY_FLAGS, Mode::empty()) {
        Ok(directory) => directory,
        Err(Errno::NOENT) => return FolderReport::default(),
        Err(error) => return unavailable("open", error),
    };
    let mut names = Vec::new();
    let mut truncated = false;
    let entries = match Dir::read_from(&directory) {
        Ok(entries) => entries,
        Err(error) => return unavailable("list", error),
    };
    for entry in entries {
        let name = match entry {
            Ok(entry) => entry.file_name().to_bytes().to_vec(),
            Err(error) => return unavailable("list", error),
        };
        if name == b"." || name == b".." {
            continue;
        }
        if names.len() == MAX_ENTRIES {
            truncated = true;
            break;
        }
        names.push(name);
    }
    names.sort();
    truncated |= names.len() > MAX_FOLDERS;
    names.truncate(MAX_FOLDERS);
    let report_deadline = Instant::now() + REPORT_BUDGET;
    let folder_count = names.len();
    let folders = names
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let folders_left = u32::try_from(folder_count - index).unwrap_or(u32::MAX);
            let fair_share = fair_time_share(report_deadline, folders_left);
            let mut budget = Budget::new(MAX_ENTRIES, fair_share.min(SCAN_BUDGET));
            let measurement = measure_folder(&directory, &name, &mut budget);
            let name = std::ffi::OsStr::from_bytes(&name);
            FolderStorage {
                name: name.to_string_lossy().into_owned(),
                path: parent.join(name),
                measurement,
            }
        })
        .collect();
    FolderReport {
        folders,
        truncated,
        unavailable: None,
    }
}

fn measure_folder(parent: &OwnedFd, name: &[u8], budget: &mut Budget) -> TargetSize {
    if let Err(reason) = budget.check() {
        return TargetSize::Unavailable { reason };
    }
    let folder = match open_directory(parent, name) {
        Ok(folder) => folder,
        Err(Errno::NOENT) => return TargetSize::Missing,
        Err(Errno::NOTDIR | Errno::LOOP) => {
            return TargetSize::Unavailable {
                reason: "not a folder".to_owned(),
            };
        }
        Err(error) => {
            return TargetSize::Unavailable {
                reason: reason(error),
            };
        }
    };
    let mut walk = Walk {
        budget,
        bytes: 0,
        issue: None,
    };
    if let Err(error) = walk.directory(folder, 0, &mut || {}) {
        walk.record(error);
    }
    match walk.issue {
        Some(reason) => TargetSize::Incomplete {
            apparent_bytes: walk.bytes,
            reason,
        },
        None => TargetSize::Complete {
            apparent_bytes: walk.bytes,
        },
    }
}

struct Budget {
    limit: usize,
    remaining: usize,
    deadline: Instant,
}

impl Budget {
    fn new(limit: usize, duration: Duration) -> Self {
        Self {
            limit,
            remaining: limit,
            deadline: Instant::now() + duration,
        }
    }

    fn check(&self) -> Result<(), String> {
        let visited = self.limit - self.remaining;
        if Instant::now() >= self.deadline {
            Err(format!("clone time budget reached after {visited} entries"))
        } else if self.remaining == 0 {
            Err(format!(
                "clone directory-entry limit reached after {visited} entries"
            ))
        } else {
            Ok(())
        }
    }
}

/// The hook lets isolated tests replace directories between inspection and opening them.
/// Production passes a no-op; every open still enforces its own no-follow boundary.
fn measure(stored: &StoredCheckout, budget: &mut Budget, between: &mut dyn FnMut()) -> TargetSize {
    match measure_inner(stored, budget, between) {
        Ok(size) => size,
        Err(reason) => TargetSize::Unavailable { reason },
    }
}

fn measure_inner(
    stored: &StoredCheckout,
    budget: &mut Budget,
    between: &mut dyn FnMut(),
) -> Result<TargetSize, String> {
    budget.check()?;
    if stored.checkout.state != CheckoutState::Live {
        return Err("checkout removal is in progress".to_owned());
    }
    let root = open_root(&stored.checkout.path)?;
    verify_marker(stored, &root)?;
    between();
    let result = match open_directory(&root, "target") {
        Ok(target) => {
            let target_identity = fs::fstat(&target).map_err(reason)?;
            let mut walk = Walk {
                budget,
                bytes: 0,
                issue: None,
            };
            // Keep the original descriptor until after the comparison below, preventing
            // inode reuse if the target is removed as the walk finishes.
            let walking = target.try_clone().map_err(|error| error.to_string())?;
            if let Err(error) = walk.directory(walking, 0, between) {
                walk.record(error);
            }
            // A target swapped while traversing it is not a complete observation of the
            // current target. Do not follow its replacement while checking.
            match fs::statat(&root, "target", AtFlags::SYMLINK_NOFOLLOW) {
                Ok(current) if same_directory(&target_identity, &current) => {}
                _ => walk.record("target directory changed during measurement".to_owned()),
            }
            match walk.issue {
                Some(reason) => TargetSize::Incomplete {
                    apparent_bytes: walk.bytes,
                    reason,
                },
                None => TargetSize::Complete {
                    apparent_bytes: walk.bytes,
                },
            }
        }
        Err(Errno::NOENT) => TargetSize::Missing,
        Err(error) => {
            return Err(format!(
                "target directory cannot be opened without following links: {error}"
            ));
        }
    };
    // Re-open from /, one segment at a time: an ancestor replacement cannot redirect this
    // check. Holding the first descriptor prevents inode reuse from hiding replacement.
    let current = open_root(&stored.checkout.path)?;
    verify_marker(stored, &current)?;
    if !same_directory(
        &fs::fstat(&root).map_err(reason)?,
        &fs::fstat(&current).map_err(reason)?,
    ) {
        return Err("checkout directory changed during measurement".to_owned());
    }
    Ok(result)
}

fn same_directory(first: &fs::Stat, second: &fs::Stat) -> bool {
    first.st_dev == second.st_dev
        && first.st_ino == second.st_ino
        && FileType::from_raw_mode(second.st_mode).is_dir()
}

fn open_directory(parent: impl AsFd, name: impl rustix::path::Arg) -> Result<OwnedFd, Errno> {
    fs::openat(parent, name, DIRECTORY_FLAGS, Mode::empty())
}

pub(crate) fn open_root(path: &Path) -> Result<OwnedFd, String> {
    if !path.is_absolute() {
        return Err("registered checkout path is not absolute".to_owned());
    }
    let mut directory = fs::open("/", ANCHOR_FLAGS, Mode::empty())
        .map_err(|error| format!("open root directory: {error}"))?;
    for (depth, component) in path.components().enumerate() {
        if depth > MAX_DEPTH {
            return Err("checkout path depth limit reached".to_owned());
        }
        match component {
            Component::RootDir => {}
            Component::Normal(name) => {
                directory = fs::openat(&directory, name, ANCHOR_FLAGS, Mode::empty())
                    .map_err(|error| format!("open checkout ancestor: {error}"))?
            }
            _ => return Err("registered checkout path is not canonical".to_owned()),
        }
    }
    Ok(directory)
}

pub(crate) fn verify_marker(stored: &StoredCheckout, root: &OwnedFd) -> Result<(), String> {
    let git = fs::openat(root, ".git", ANCHOR_FLAGS, Mode::empty())
        .map_err(|error| format!("open identity directory: {error}"))?;
    let marker = stored.marker_name();
    // Even a malformed stored marker must remain one name relative to this descriptor.
    if marker.components().count() != 1 || marker.file_name() != Some(marker.as_os_str()) {
        return Err("invalid recorded checkout marker".to_owned());
    }
    let stat = fs::statat(&git, &marker, AtFlags::SYMLINK_NOFOLLOW).map_err(reason)?;
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err("checkout identity marker is not a regular file".to_owned());
    }
    Ok(())
}

struct Walk<'a> {
    budget: &'a mut Budget,
    bytes: u64,
    issue: Option<String>,
}

impl Walk<'_> {
    fn record(&mut self, reason: String) {
        self.issue.get_or_insert(reason);
    }

    fn directory(
        &mut self,
        directory: OwnedFd,
        depth: usize,
        between: &mut dyn FnMut(),
    ) -> Result<(), String> {
        let mut entries = Dir::new(directory).map_err(reason)?;
        // Checked after each read so a directory that ends exactly at the limit is complete.
        while let Some(entry) = entries.read() {
            self.budget.check()?;
            self.budget.remaining -= 1;
            let entry = entry.map_err(reason)?;
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            let parent = entries.fd().map_err(reason)?;
            let stat = match fs::statat(parent, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => stat,
                Err(error) => {
                    self.record(reason(error));
                    continue;
                }
            };
            match FileType::from_raw_mode(stat.st_mode) {
                FileType::RegularFile => {
                    let size =
                        u64::try_from(stat.st_size).map_err(|_| "negative file size".to_owned())?;
                    self.bytes = self
                        .bytes
                        .checked_add(size)
                        .ok_or("apparent byte count overflow")?;
                }
                FileType::Directory => {
                    if depth >= MAX_DEPTH {
                        self.record("directory depth limit reached".to_owned());
                        continue;
                    }
                    between();
                    match open_directory(parent, name) {
                        Ok(child) => {
                            let opened = fs::fstat(&child).map_err(reason)?;
                            if !same_directory(&stat, &opened) {
                                self.record("directory changed before opening".to_owned());
                                continue;
                            }
                            if let Err(error) = self.directory(child, depth + 1, between) {
                                self.record(error);
                            }
                        }
                        Err(error) => self.record(reason(error)),
                    }
                }
                FileType::Symlink => self.record("symbolic links are not followed".to_owned()),
                _ => self.record("non-regular files are not counted".to_owned()),
            }
        }
        Ok(())
    }
}

fn reason(error: Errno) -> String {
    format!("filesystem measurement failed: {error}")
}

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
