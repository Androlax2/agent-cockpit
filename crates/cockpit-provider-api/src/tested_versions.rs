//! The harness versions an adapter's conformance tests pass against (#732).
//!
//! Each adapter ships a `tested-versions.txt`: one version per line, `#` comments and blank
//! lines allowed. The nightly conformance job appends a line when the latest harness passes;
//! the adapter reads the file at compile time.

use crate::Provenance;

impl Provenance {
    /// `Tested` when `version` is a line of `tested_versions`, `Discovered` for any other or
    /// unknown version. `tested_versions` is an adapter's file, checked by its own test with
    /// [`parse_tested_versions`].
    pub fn for_version(version: Option<&str>, tested_versions: &str) -> Provenance {
        match version {
            Some(version) if version_lines(tested_versions).any(|(_, line)| line == version) => {
                Provenance::Tested {
                    version: version.to_owned(),
                }
            }
            _ => Provenance::Discovered,
        }
    }
}

/// The versions in an adapter's tested-versions file, in file order. Refuses a file with no
/// version, a line that is not a version, or a version listed twice, naming the line.
pub fn parse_tested_versions(tested_versions: &str) -> Result<Vec<&str>, String> {
    let mut versions: Vec<&str> = Vec::new();
    for (line_number, line) in version_lines(tested_versions) {
        if !is_version(line) {
            return Err(format!("line {line_number}: {line:?} is not a version"));
        }
        if versions.contains(&line) {
            return Err(format!("line {line_number}: {line} is listed twice"));
        }
        versions.push(line);
    }
    if versions.is_empty() {
        return Err("no version is listed".to_owned());
    }
    Ok(versions)
}

/// Each non-comment, non-blank line with its 1-based number, trimmed.
fn version_lines(tested_versions: &str) -> impl Iterator<Item = (usize, &str)> {
    tested_versions
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
}

/// A dotted version such as `2.1.316`, optionally with a pre-release suffix (`0.167.0-alpha.1`).
fn is_version(line: &str) -> bool {
    let (release, suffix) = match line.split_once('-') {
        Some((release, suffix)) => (release, Some(suffix)),
        None => (line, None),
    };
    let is_pre_release = |suffix: &str| {
        suffix
            .split(['.', '-'])
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
    };
    release.split('.').count() >= 2
        && release
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && suffix.is_none_or(is_pre_release)
}
