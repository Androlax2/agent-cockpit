//! Artifact fingerprints: the SHA-256 of a file inside a checkout. Reading files is the only
//! I/O here besides the database; no program runs.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::invalid;
use crate::{ARTIFACTS_MAX, Artifact, PATH_MAX_CHARS, Refusal};

/// The fingerprints of `paths`, relative to `checkout`, ordered by path. Take them before
/// the run starts. Refused unless each names an existing file inside the checkout, once.
pub fn fingerprint(checkout: &Path, paths: &[&str]) -> Result<Vec<Artifact>, Refusal> {
    let mut sorted = paths.to_vec();
    sorted.sort_unstable();
    check_artifact_paths(&sorted)?;
    let checkout = Checkout::open(checkout)?;
    sorted
        .into_iter()
        .map(|path| {
            let digest = checkout
                .digest(path)
                .map_err(|reason| invalid("artifacts", format!("{path}: {reason}")))?
                .ok_or_else(|| invalid("artifacts", format!("{path}: does not exist")))?;
            Ok(Artifact {
                path: path.to_owned(),
                digest,
            })
        })
        .collect()
}

/// Refuses more than [`ARTIFACTS_MAX`] paths, a path that is not plainly relative, and one
/// named twice. `sorted` is ordered, so a repeat is adjacent.
pub(crate) fn check_artifact_paths(sorted: &[&str]) -> Result<(), Refusal> {
    if sorted.len() > ARTIFACTS_MAX {
        return Err(invalid("artifacts", format!("at most {ARTIFACTS_MAX}")));
    }
    for path in sorted {
        check_relative(path).map_err(|reason| invalid("artifacts", format!("{path}: {reason}")))?;
    }
    if let Some(pair) = sorted.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(invalid("artifacts", format!("{} is named twice", pair[0])));
    }
    Ok(())
}

/// A checkout root, resolved once.
pub(crate) struct Checkout {
    root: PathBuf,
}

impl Checkout {
    /// Refused unless `path` resolves to a directory, so a wrong checkout is named as such
    /// instead of showing every artifact as missing.
    pub(crate) fn open(path: &Path) -> Result<Checkout, Refusal> {
        let root = path
            .canonicalize()
            .map_err(|error| invalid("checkout", format!("cannot be resolved: {error}")))?;
        if !root.is_dir() {
            return Err(invalid("checkout", "is not a directory"));
        }
        Ok(Checkout { root })
    }

    /// The digest of `path` now: `None` when nothing is there, the reason when it is not a
    /// readable file inside the checkout.
    pub(crate) fn digest(&self, path: &str) -> Result<Option<String>, String> {
        check_relative(path)?;
        let full = match self.root.join(path).canonicalize() {
            Ok(full) => full,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        // Symbolic links are followed, so one pointing out of the checkout is caught here.
        if !full.starts_with(&self.root) {
            return Err("resolves outside the checkout".to_owned());
        }
        if !full.is_file() {
            return Err("is not a file".to_owned());
        }
        sha256_of(&full)
            .map(Some)
            .map_err(|error| error.to_string())
    }
}

fn sha256_of(path: &Path) -> io::Result<String> {
    let mut hasher = Sha256::new();
    io::copy(&mut File::open(path)?, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Refuses an empty, absolute, too long or non-normalized path: `/`-separated segments,
/// none empty, `.` or `..`, so one file has one spelling.
pub(crate) fn check_relative(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("is empty".to_owned());
    }
    if path.chars().count() > PATH_MAX_CHARS {
        return Err(format!("is longer than {PATH_MAX_CHARS} characters"));
    }
    let is_normalized = path
        .split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
    if !is_normalized {
        return Err(
            "must be relative to the checkout, `/`-separated, without empty, `.` or `..` segments"
                .to_owned(),
        );
    }
    Ok(())
}
