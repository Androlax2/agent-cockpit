//! Artifact fingerprints: the SHA-256 of a file inside a checkout. Reading files is the only
//! I/O here besides the database; no program runs.

use std::fs::File;
use std::io;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::invalid;
use crate::{Artifact, Refusal};

/// The fingerprints of `paths`, relative to `checkout`, ordered by path. Take them before
/// the run starts. Refused unless each names an existing file inside the checkout.
pub fn fingerprint(checkout: &Path, paths: &[&str]) -> Result<Vec<Artifact>, Refusal> {
    let mut artifacts = paths
        .iter()
        .map(|path| {
            let digest = file_digest(checkout, path)
                .map_err(|reason| invalid("artifacts", format!("{path}: {reason}")))?
                .ok_or_else(|| invalid("artifacts", format!("{path}: does not exist")))?;
            Ok(Artifact {
                path: (*path).to_owned(),
                digest,
            })
        })
        .collect::<Result<Vec<_>, Refusal>>()?;
    artifacts.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(artifacts)
}

/// The digest of `path` now: `None` when nothing is there, the reason when it is not a
/// readable file inside the checkout.
pub(crate) fn file_digest(checkout: &Path, path: &str) -> Result<Option<String>, String> {
    check_relative(path, false)?;
    let full = match checkout.join(path).canonicalize() {
        Ok(full) => full,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let root = checkout
        .canonicalize()
        .map_err(|error| format!("the checkout cannot be resolved: {error}"))?;
    // Symbolic links are followed, so one pointing out of the checkout is caught here.
    if !full.starts_with(&root) {
        return Err("resolves outside the checkout".to_owned());
    }
    if !full.is_file() {
        return Err("is not a file".to_owned());
    }
    sha256_of(&full)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn sha256_of(path: &PathBuf) -> io::Result<String> {
    let mut hasher = Sha256::new();
    io::copy(&mut File::open(path)?, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Refuses an empty, absolute or parent-reaching path. `.` names the root only where
/// `allows_root`.
pub(crate) fn check_relative(path: &str, allows_root: bool) -> Result<(), String> {
    if path.is_empty() {
        return Err("is empty".to_owned());
    }
    if allows_root && path == "." {
        return Ok(());
    }
    let is_plain = Path::new(path)
        .components()
        .all(|component| matches!(component, Component::Normal(_)));
    if !is_plain {
        return Err("must be relative to the checkout, without `..` or `.`".to_owned());
    }
    Ok(())
}
