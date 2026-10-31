use std::fmt;
use std::path::Path;
use std::process::Command;

use crate::Error;

/// A tmux release, compared by major and minor number; letter suffixes (`3.3a`) are fixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct TmuxVersion {
    pub major: u32,
    pub minor: u32,
}

/// tmux 3.2 added the control-mode client flags this crate relies on: `pause-after` (flow
/// control), `read-only` and `ignore-size` on `attach-session -f`.
pub const MINIMUM_VERSION: TmuxVersion = TmuxVersion { major: 3, minor: 2 };

impl fmt::Display for TmuxVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.major, self.minor)
    }
}

impl TmuxVersion {
    /// Reads the line `tmux -V` prints: `tmux 3.4`, `tmux 3.3a`, `tmux next-3.6`,
    /// `tmux 3.2-rc2`. Anything else (`tmux master`, `tmux openbsd-7.5`) is `None`: its
    /// features cannot be known from the name.
    pub fn parse(printed: &str) -> Option<TmuxVersion> {
        let release = printed.trim().strip_prefix("tmux ")?;
        let release = release.strip_prefix("next-").unwrap_or(release);
        let (major, rest) = release.split_once('.')?;
        let minor_digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let suffix = &rest[minor_digits..];
        let is_known_suffix = suffix.is_empty()
            || suffix.chars().all(|c| c.is_ascii_lowercase())
            || suffix.starts_with("-rc");
        if !is_known_suffix {
            return None;
        }
        Some(TmuxVersion {
            major: major.parse().ok()?,
            minor: rest[..minor_digits].parse().ok()?,
        })
    }
}

/// The readiness check: runs `<binary> -V` and fails unless it is tmux [`MINIMUM_VERSION`]
/// or newer.
pub fn check_version(binary: &Path) -> Result<TmuxVersion, Error> {
    let output =
        Command::new(binary)
            .arg("-V")
            .output()
            .map_err(|source| Error::TmuxNotRunnable {
                binary: binary.to_path_buf(),
                source,
            })?;
    let printed = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() {
        return Err(Error::CommandFailed {
            arguments: vec!["-V".into()],
            status: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    let version = TmuxVersion::parse(&printed).ok_or_else(|| Error::UnrecognizedVersion {
        binary: binary.to_path_buf(),
        printed: printed.clone(),
    })?;
    if version < MINIMUM_VERSION {
        return Err(Error::TmuxTooOld {
            binary: binary.to_path_buf(),
            found: printed.trim_start_matches("tmux ").to_string(),
        });
    }
    Ok(version)
}
