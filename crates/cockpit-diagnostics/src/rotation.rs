use std::fs::{self, DirBuilder, File};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::Error;
use crate::error::io_error;

pub(crate) const LOG_FILE: &str = "cockpit.log";

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
    size: u64,
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
        let (file, size) =
            open_current(log_dir).map_err(io_error("open the log file in", log_dir))?;
        let rotating = Self {
            log_dir: log_dir.to_path_buf(),
            limits,
            file,
            size,
        };
        rotating
            .remove_expired()
            .map_err(io_error("remove old log files in", log_dir))?;
        Ok(rotating)
    }

    /// Writes one whole record. Returns `false` when the record was dropped: larger than a
    /// file, or the disk refused it.
    pub(crate) fn write_record(&mut self, record: &[u8]) -> bool {
        let length = record.len() as u64;
        if length > self.limits.max_file_bytes {
            return false;
        }
        if self.size + length > self.limits.max_file_bytes && self.rotate().is_err() {
            return false;
        }
        match self.file.write_all(record) {
            Ok(()) => {
                self.size += length;
                true
            }
            Err(_) => false,
        }
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
            fs::remove_file(path(0))?;
        }
        for number in (0..oldest).rev() {
            match fs::rename(path(number), path(number + 1)) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                result => result?,
            }
        }
        let (file, size) = open_current(&self.log_dir)?;
        self.file = file;
        self.size = size;
        self.remove_expired()
    }

    /// Removes rotated files past the age limit or beyond the file count.
    fn remove_expired(&self) -> io::Result<()> {
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
