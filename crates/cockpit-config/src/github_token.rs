//! A project's GitHub token (#754): a fine-grained token the owner keeps in a file of their
//! configuration directory, one per project, which the daemon reads at each launch and gives
//! that project's agents as `GH_TOKEN`. The value never leaves the file except into a
//! launch's private environment: errors name the file and what is wrong, never its contents.

use std::fmt;
use std::io::{ErrorKind, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::Layout;

/// The folder of [`Layout::config_dir`] that holds one token file per project, named as the
/// project is.
pub const GITHUB_TOKENS_DIR: &str = "github-tokens";

/// The variable a launch sets to its project's token, the one `gh` and its Git credential
/// helper read first.
pub const GITHUB_TOKEN_VARIABLE: &str = "GH_TOKEN";

/// Every variable `gh` reads a token from. A launch never inherits one: it sets
/// [`GITHUB_TOKEN_VARIABLE`] to its project's token, or none of them.
pub const GITHUB_TOKEN_VARIABLES: [&str; 4] = [
    "GH_TOKEN",
    "GITHUB_TOKEN",
    "GH_ENTERPRISE_TOKEN",
    "GITHUB_ENTERPRISE_TOKEN",
];

/// Far above a fine-grained token's 93 characters; a larger file is not a token.
const MAX_TOKEN_BYTES: u64 = 1024;

impl Layout {
    /// Where project `project`'s GitHub token lives. The project name is the registry's,
    /// which never holds a `/` or starts with `.`.
    pub fn github_token_file(&self, project: &str) -> PathBuf {
        self.config_dir.value.join(GITHUB_TOKENS_DIR).join(project)
    }
}

/// A token read from its file. It has no `Display`, and its `Debug` is redacted, so a log or
/// error that formats it shows no value.
#[derive(Clone, PartialEq, Eq)]
pub struct GithubToken(String);

impl GithubToken {
    /// The value, for the launch's environment alone.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for GithubToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GithubToken([redacted])")
    }
}

/// Why a token file cannot be used. Each says what to fix; none carries the file's contents.
#[derive(Debug)]
pub enum GithubTokenError {
    Unreadable {
        path: PathBuf,
        error: String,
    },
    NotAFile {
        path: PathBuf,
    },
    /// Other users may read or write it: mode `0600` or `0400` only.
    TooOpen {
        path: PathBuf,
        mode: u32,
    },
    Malformed {
        path: PathBuf,
        reason: &'static str,
    },
}

impl GithubTokenError {
    pub fn path(&self) -> &Path {
        match self {
            Self::Unreadable { path, .. }
            | Self::NotAFile { path }
            | Self::TooOpen { path, .. }
            | Self::Malformed { path, .. } => path,
        }
    }
}

impl fmt::Display for GithubTokenError {
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
                "{} has mode {mode:04o}: other users may reach it; `chmod 600` it",
                path.display()
            ),
            Self::Malformed { path, reason } => write!(formatter, "{} {reason}", path.display()),
        }
    }
}

impl std::error::Error for GithubTokenError {}

/// The token in `path`, or `None` when there is no such file: that project has no token.
/// Blocking: it reads the file. A symbolic link, a file other users may reach or a value
/// that cannot be a GitHub token is refused, never used.
pub fn read_github_token(path: &Path) -> Result<Option<GithubToken>, GithubTokenError> {
    let unreadable = |error: std::io::Error| GithubTokenError::Unreadable {
        path: path.to_owned(),
        error: error.kind().to_string(),
    };
    let link = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(unreadable(error)),
    };
    if !link.file_type().is_file() {
        return Err(GithubTokenError::NotAFile {
            path: path.to_owned(),
        });
    }
    let file = std::fs::File::open(path).map_err(unreadable)?;
    let opened = file.metadata().map_err(unreadable)?;
    // The file checked above is the one opened, not one swapped in between.
    if (opened.dev(), opened.ino()) != (link.dev(), link.ino()) {
        return Err(GithubTokenError::NotAFile {
            path: path.to_owned(),
        });
    }
    let mode = opened.permissions().mode() & 0o7777;
    if mode & 0o077 != 0 {
        return Err(GithubTokenError::TooOpen {
            path: path.to_owned(),
            mode,
        });
    }
    let malformed = |reason| GithubTokenError::Malformed {
        path: path.to_owned(),
        reason,
    };
    if opened.len() > MAX_TOKEN_BYTES {
        return Err(malformed("is larger than a GitHub token"));
    }
    let mut contents = String::new();
    file.take(MAX_TOKEN_BYTES + 1)
        .read_to_string(&mut contents)
        .map_err(|_| malformed("is not UTF-8 text"))?;
    let token = contents.trim_end_matches(['\n', '\r']);
    if token.is_empty() {
        return Err(malformed("is empty"));
    }
    // GitHub's tokens (`github_pat_…`, `ghp_…`) are letters, digits and `_`. Anything else,
    // a second line or a space included, is not one and would reach `gh` mangled.
    if !token
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(malformed(
            "is not one GitHub token: only letters, digits and `_`, on one line",
        ));
    }
    Ok(Some(GithubToken(token.to_owned())))
}

/// Whether project `project` has a usable token, read from `layout`, for what Torlek shows:
/// never the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GithubTokenState {
    Configured { path: PathBuf },
    Missing { path: PathBuf },
    Unusable { reason: String },
}

/// [`read_github_token`] for project `project`, as its state. Blocking.
pub fn github_token_state(layout: &Layout, project: &str) -> GithubTokenState {
    let path = layout.github_token_file(project);
    match read_github_token(&path) {
        Ok(Some(_)) => GithubTokenState::Configured { path },
        Ok(None) => GithubTokenState::Missing { path },
        Err(error) => GithubTokenState::Unusable {
            reason: error.to_string(),
        },
    }
}
