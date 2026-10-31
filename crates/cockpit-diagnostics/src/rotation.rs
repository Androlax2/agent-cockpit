use std::fs::{self, DirBuilder, File};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::Error;
use crate::error::io_error;

pub(crate) const LOG_FILE: &str = "cockpit.log";
/// Held locked while a process logs to the directory. A file opened for writing, not the
/// directory itself: NFS emulates `flock` with byte-range locks that need a writable handle.
const LOCK_FILE: &str = "cockpit.log.lock";

/// Size rotation with a total cap: at most `max_files` files of `max_file_bytes` each, and
/// rotated files older than `max_age` are removed. The defaults are the 2026-10-29 budgets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileLimits {
    pub max_file_bytes: u64,
    /// Counts `cockpit.log` itself, so `4` keeps `cockpit.log` to `cockpit.log.3`.
    pub max_files: usize,
    pub max_age: Duration,
}

impl Default for FileLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 4 * 1024 * 1024,
            max_files: 4,
            max_age: Duration::from_secs(7 * 24 * 3600),
        }
    }
}

/// The rotation number of a log file name: `cockpit.log` is 0, `cockpit.log.2` is 2.
pub(crate) fn rotation_number(file_name: &str) -> Option<usize> {
    let suffix = file_name.strip_prefix(LOG_FILE)?;
    if suffix.is_empty() {
        return Some(0);
    }
    let number = suffix.strip_prefix('.')?;
    if number.starts_with('0') || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    number.parse().ok()
}

pub(crate) struct RotatingFile {
    log_dir: PathBuf,
    limits: FileLimits,
    file: File,
    /// Bytes in `cockpit.log`.
    size: u64,
    /// Rotate before `size` passes this: `max_file_bytes`, or one file further after a
    /// failed rotation, because retrying per record would shift older files until lost.
    rotate_at: u64,
    /// Locked so a second process fails to start; released when the writer thread ends.
    _lock: File,
}

/// What [`RotatingFile::write_record`] did with a record.
pub(crate) enum Written {
    Yes,
    /// Larger than a file, or the disk refused it.
    Dropped,
    /// Written to the current file because rotating failed.
    AfterFailedRotation(io::Error),
}

impl RotatingFile {
    pub(crate) fn open(log_dir: &Path, limits: FileLimits) -> Result<Self, Error> {
        if limits.max_files == 0 {
            return Err(Error::InvalidLimits {
                reason: "max_files must be at least 1",
            });
        }
        if limits.max_file_bytes == 0 {
            return Err(Error::InvalidLimits {
                reason: "max_file_bytes must be at least 1",
            });
        }
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(log_dir)
            .map_err(io_error("create the log directory", log_dir))?;
        fs::set_permissions(log_dir, fs::Permissions::from_mode(0o700))
            .map_err(io_error("make the log directory private", log_dir))?;
        let lock_path = log_dir.join(LOCK_FILE);
        let lock = File::options()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)
            .map_err(io_error("open the log lock", &lock_path))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                return Err(Error::LogDirInUse {
                    path: log_dir.to_path_buf(),
                });
            }
            Err(fs::TryLockError::Error(source)) => {
                return Err(io_error("lock", lock_path)(source));
            }
        }
        let (file, size) =
            open_current(log_dir).map_err(io_error("open the log file in", log_dir))?;
        let rotating = Self {
            log_dir: log_dir.to_path_buf(),
            limits,
            file,
            size,
            rotate_at: limits.max_file_bytes,
            _lock: lock,
        };
        rotating
            .remove_expired()
            .map_err(io_error("remove old log files in", log_dir))?;
        Ok(rotating)
    }

    /// Writes one whole record, rotating first when it would not fit.
    pub(crate) fn write_record(&mut self, record: &[u8]) -> Written {
        let length = record.len() as u64;
        if length > self.limits.max_file_bytes {
            return Written::Dropped;
        }
        let mut rotation_error = None;
        if self.size + length > self.rotate_at
            && let Err(error) = self.rotate()
        {
            self.rotate_at = self.size + self.limits.max_file_bytes;
            rotation_error = Some(error);
        }
        if self.file.write_all(record).is_err() {
            return Written::Dropped;
        }
        self.size += length;
        rotation_error.map_or(Written::Yes, Written::AfterFailedRotation)
    }

    pub(crate) fn flush(&mut self) {
        // Nothing to report to: the file is unbuffered, so this only asks the OS to sync.
        let _ = self.file.sync_data();
    }

    fn rotate(&mut self) -> io::Result<()> {
        let path = |number: usize| match number {
            0 => self.log_dir.join(LOG_FILE),
            _ => self.log_dir.join(format!("{LOG_FILE}.{number}")),
        };
        let oldest = self.limits.max_files - 1;
        if oldest == 0 {
            // Emptying in place keeps a working handle; unlinking could leave none.
            self.file.set_len(0)?;
            self.size = 0;
            return Ok(());
        }
        for number in (0..oldest).rev() {
            match fs::rename(path(number), path(number + 1)) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                result => result?,
            }
        }
        let (file, size) = match open_current(&self.log_dir) {
            Ok(opened) => opened,
            Err(error) => {
                // The handle now names cockpit.log.1: move it back so `size` stays true and
                // the next retry does not rotate the file being written.
                let _ = fs::rename(path(1), path(0));
                return Err(error);
            }
        };
        self.file = file;
        self.size = size;
        self.rotate_at = self.limits.max_file_bytes;
        Ok(())
    }

    /// Removes rotated files past the age limit or beyond the file count. Rotation itself
    /// keeps the count; the writer calls this hourly for the age limit.
    pub(crate) fn remove_expired(&self) -> io::Result<()> {
        let oldest_kept = SystemTime::now()
            .checked_sub(self.limits.max_age)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        for entry in fs::read_dir(&self.log_dir)? {
            let entry = entry?;
            let Some(number) = rotation_number(&entry.file_name().to_string_lossy()) else {
                continue;
            };
            if number == 0 {
                continue;
            }
            let is_beyond_count = number >= self.limits.max_files;
            let is_too_old = entry.metadata()?.modified()? < oldest_kept;
            if is_beyond_count || is_too_old {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
}

/// Opens `cockpit.log` for appending and makes it private, even when it already existed.
fn open_current(log_dir: &Path) -> io::Result<(File, u64)> {
    let file = File::options()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(log_dir.join(LOG_FILE))?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    let size = file.metadata()?.len();
    Ok((file, size))
}
