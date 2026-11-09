//! A secret the owner keeps in a file of their configuration directory (#754, #968): read so
//! that a wrong file is refused and its contents never reach an error. A GitHub token and a
//! provider's credential are read by the same rules, each with its own alphabet.

use std::fmt;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// What one kind of secret looks like.
pub(crate) struct TokenRule {
    /// Far above the longest real value; a larger file is not one.
    pub max_bytes: u64,
    /// Said of a file larger than [`TokenRule::max_bytes`].
    pub too_large: &'static str,
    pub is_token_character: fn(char) -> bool,
    /// Said of a file holding any other character, a second line or a space included.
    pub not_one: &'static str,
}

/// Why a token file cannot be used. Each says what to fix; none carries the file's contents.
#[derive(Debug)]
pub enum TokenFileError {
    Unreadable {
        path: PathBuf,
        error: String,
    },
    NotAFile {
        path: PathBuf,
    },
    /// Its mode is not `0600` or `0400`: others may reach it, or it is executable or setuid.
    TooOpen {
        path: PathBuf,
        mode: u32,
    },
    Malformed {
        path: PathBuf,
        reason: &'static str,
    },
}

impl TokenFileError {
    pub fn path(&self) -> &Path {
        match self {
            Self::Unreadable { path, .. }
            | Self::NotAFile { path }
            | Self::TooOpen { path, .. }
            | Self::Malformed { path, .. } => path,
        }
    }
}

impl fmt::Display for TokenFileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreadable { path, error } => {
                write!(formatter, "{} cannot be read: {error}", path.display())
            }
            Self::NotAFile { path } => write!(
                formatter,
                "{} is not a regular file; a symbolic link is refused too",
                path.display()
            ),
            Self::TooOpen { path, mode } => write!(
                formatter,
                "{} has mode {mode:04o}, not 0600 or 0400; `chmod 600` it",
                path.display()
            ),
            Self::Malformed { path, reason } => write!(formatter, "{} {reason}", path.display()),
        }
    }
}

impl std::error::Error for TokenFileError {}

/// The secret in `path` as `rule` reads one, or `None` when there is no such file. Blocking:
/// it reads the file. A symbolic link, a file other users may reach or a value `rule` does not
/// accept is refused, never used.
pub(crate) fn read_token_file(
    path: &Path,
    rule: &TokenRule,
) -> Result<Option<String>, TokenFileError> {
    let unreadable = |error: std::io::Error| TokenFileError::Unreadable {
        path: path.to_owned(),
        error: error.kind().to_string(),
    };
    let link = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(error)),
    };
    if !link.file_type().is_file() {
        return Err(TokenFileError::NotAFile {
            path: path.to_owned(),
        });
    }
    let file = std::fs::File::open(path).map_err(unreadable)?;
    let opened = file.metadata().map_err(unreadable)?;
    // The file checked above is the one opened, not one swapped in between.
    if (opened.dev(), opened.ino()) != (link.dev(), link.ino()) {
        return Err(TokenFileError::NotAFile {
            path: path.to_owned(),
        });
    }
    let mode = opened.permissions().mode() & 0o7777;
    if mode & !0o600 != 0 {
        return Err(TokenFileError::TooOpen {
            path: path.to_owned(),
            mode,
        });
    }
    let malformed = |reason| TokenFileError::Malformed {
        path: path.to_owned(),
        reason,
    };
    if opened.len() > rule.max_bytes {
        return Err(malformed(rule.too_large));
    }
    let mut contents = String::new();
    file.take(rule.max_bytes + 1)
        .read_to_string(&mut contents)
        .map_err(|error| match error.kind() {
            ErrorKind::InvalidData => malformed("is not UTF-8 text"),
            _ => unreadable(error),
        })?;
    // The file grew after its size was checked.
    if contents.len() as u64 > rule.max_bytes {
        return Err(malformed(rule.too_large));
    }
    let token = contents.trim_end_matches(['\n', '\r']);
    if token.is_empty() {
        return Err(malformed("is empty"));
    }
    // Anything else would reach a header or a client mangled.
    if !token.chars().all(rule.is_token_character) {
        return Err(malformed(rule.not_one));
    }
    Ok(Some(token.to_owned()))
}

/// Why a secret cannot be saved or removed (#1251). None carries the value.
#[derive(Debug)]
pub enum TokenWriteError {
    /// The value is not one secret of its kind: `reason` says why.
    NotOne {
        reason: &'static str,
    },
    /// A folder or file on the way is a symbolic link or not of its kind.
    NotPlain {
        path: PathBuf,
    },
    Io {
        path: PathBuf,
        error: String,
    },
}

impl fmt::Display for TokenWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotOne { reason } => write!(formatter, "the value {reason}"),
            Self::NotPlain { path } => write!(
                formatter,
                "{} is a symbolic link or not a plain file or folder; remove it first",
                path.display()
            ),
            Self::Io { path, error } => write!(formatter, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for TokenWriteError {}

/// Saves `value` alone, on one line, in `path` with mode `0600`, as the user guide says to
/// by hand: its folder is created with mode `0700` when missing. Blocking. The file is
/// written beside `path` and renamed over it, so a reader sees the old secret or the new
/// one, never part of one. A value `rule` would refuse to read is refused here.
pub(crate) fn save_token_file(
    path: &Path,
    value: &str,
    rule: &TokenRule,
) -> Result<(), TokenWriteError> {
    if value.is_empty() {
        return Err(TokenWriteError::NotOne { reason: "is empty" });
    }
    // The file holds the value and its newline, and the read refuses a larger file.
    if value.len() as u64 + 1 > rule.max_bytes {
        return Err(TokenWriteError::NotOne {
            reason: rule.too_large,
        });
    }
    if !value.chars().all(rule.is_token_character) {
        return Err(TokenWriteError::NotOne {
            reason: rule.not_one,
        });
    }
    let io = |path: &Path| {
        let path = path.to_owned();
        move |error: std::io::Error| TokenWriteError::Io {
            path,
            error: error.to_string(),
        }
    };
    let folder = path.parent().ok_or_else(|| TokenWriteError::NotPlain {
        path: path.to_owned(),
    })?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(folder)
        .map_err(io(folder))?;
    if !std::fs::symlink_metadata(folder)
        .map_err(io(folder))?
        .file_type()
        .is_dir()
    {
        return Err(TokenWriteError::NotPlain {
            path: folder.to_owned(),
        });
    }
    if let Ok(existing) = std::fs::symlink_metadata(path)
        && !existing.file_type().is_file()
    {
        return Err(TokenWriteError::NotPlain {
            path: path.to_owned(),
        });
    }
    let file_name = path.file_name().ok_or_else(|| TokenWriteError::NotPlain {
        path: path.to_owned(),
    })?;
    let mut staged_name = std::ffi::OsString::from(".");
    staged_name.push(file_name);
    staged_name.push(format!(
        ".{}.{}.saving",
        std::process::id(),
        SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let staged = folder.join(staged_name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)
        .map_err(io(&staged))?;
    // Only a file this call created is removed on failure.
    let written = write_staged(&mut file, value).and_then(|()| std::fs::rename(&staged, path));
    if let Err(error) = written {
        let _ = std::fs::remove_file(&staged);
        return Err(io(path)(error));
    }
    Ok(())
}

/// Numbers this process's saves, so two at once never share a staged file.
static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Writes `value` and its newline to the staged `file`, which `create_new` made: it never
/// follows or reuses a planted file. The mode is set again because the umask may have
/// narrowed it to `0400`, never widened it.
fn write_staged(file: &mut std::fs::File, value: &str) -> std::io::Result<()> {
    file.write_all(value.as_bytes())?;
    file.write_all(b"\n")?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.sync_all()
}

/// Removes the secret in `path`; `false` when there was none. Blocking. A folder there is
/// refused; a symbolic link is removed, never what it points to.
pub fn remove_secret_file(path: &Path) -> Result<bool, TokenWriteError> {
    let io = |error: std::io::Error| TokenWriteError::Io {
        path: path.to_owned(),
        error: error.to_string(),
    };
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io(error)),
        Ok(metadata) if metadata.file_type().is_dir() => {
            return Err(TokenWriteError::NotPlain {
                path: path.to_owned(),
            });
        }
        Ok(_) => {}
    }
    std::fs::remove_file(path).map_err(io)?;
    Ok(true)
}

/// The permission bits of the file at `path`, read without following a symbolic link;
/// `None` when nothing is there. For what Torlek shows: never the contents.
pub fn token_file_mode(path: &Path) -> Option<u32> {
    std::fs::symlink_metadata(path)
        .ok()
        .map(|metadata| metadata.permissions().mode() & 0o7777)
}
