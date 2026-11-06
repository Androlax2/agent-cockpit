//! A secret the owner keeps in a file of their configuration directory (#754, #968): read so
//! that a wrong file is refused and its contents never reach an error. A GitHub token and a
//! provider's credential are read by the same rules, each with its own alphabet.

use std::fmt;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
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
