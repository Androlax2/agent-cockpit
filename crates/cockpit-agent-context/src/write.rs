//! Writing the bundle where agents and harnesses read it. The daemon does it at every start
//! (Herdr's model, #49): the files on disk always match the running release, and a hand edit
//! lasts until the next start.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::bundle::documents;
use crate::manifest::{Manifest, manifest};

/// The manifest's file, beside the `cockpit` skill directory.
pub const MANIFEST_FILE: &str = "manifest.json";

#[derive(Debug)]
pub enum WriteError {
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    /// A directory of the bundle is something else, a symbolic link included: it is refused,
    /// never followed, so the write cannot land outside the bundle.
    NotADirectory { path: PathBuf },
}

impl fmt::Display for WriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WriteError::Io {
                action,
                path,
                source,
            } => write!(formatter, "cannot {action} {}: {source}", path.display()),
            WriteError::NotADirectory { path } => write!(
                formatter,
                "{} is not a directory (a symbolic link is refused); remove it so the \
                 instruction bundle can be written",
                path.display()
            ),
        }
    }
}

impl std::error::Error for WriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WriteError::Io { source, .. } => Some(source),
            WriteError::NotADirectory { .. } => None,
        }
    }
}

/// Writes every document and the manifest under `dir`, replacing what is there, and removes
/// files an older release left in the bundle's directories. Each file is replaced by a rename,
/// so a reader sees the old or the new text, never half of one. Returns the manifest written,
/// naming the earlier bundle it superseded when `dir` held another release's.
pub fn write_bundle(dir: &Path) -> Result<Manifest, WriteError> {
    let mut written = manifest();
    written.supersedes = previous_manifest_id(dir).filter(|previous| *previous != written.id);
    let mut expected: BTreeSet<PathBuf> = BTreeSet::new();
    for document in documents() {
        let path = dir.join(document.path);
        let parent = path.parent().expect("a document path names a file");
        ensure_dir(dir, parent)?;
        replace(&path, document.file_text().as_bytes())?;
        expected.insert(path);
    }
    let directories: BTreeSet<PathBuf> = expected
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();
    for directory in directories {
        remove_strays(&directory, &expected)?;
    }
    let text = serde_json::to_string_pretty(&written).expect("a manifest serializes");
    replace(&dir.join(MANIFEST_FILE), format!("{text}\n").as_bytes())?;
    Ok(written)
}

/// The id in the manifest a previous start wrote; `None` when there is none or it cannot be
/// read, which only loses the `supersedes` note: the bundle is rewritten either way.
fn previous_manifest_id(dir: &Path) -> Option<String> {
    let text = fs::read_to_string(dir.join(MANIFEST_FILE)).ok()?;
    let previous: serde_json::Value = serde_json::from_str(&text).ok()?;
    previous.get("id")?.as_str().map(str::to_owned)
}

/// Creates `target` and every directory between `root` and it, refusing any that exists as
/// something other than a real directory.
fn ensure_dir(root: &Path, target: &Path) -> Result<(), WriteError> {
    let relative = target
        .strip_prefix(root)
        .expect("a document's directory lies under the bundle directory");
    let mut current = root.to_path_buf();
    check_or_create(&current)?;
    for component in relative.components() {
        current.push(component);
        check_or_create(&current)?;
    }
    Ok(())
}

fn check_or_create(path: &Path) -> Result<(), WriteError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(WriteError::NotADirectory {
            path: path.to_path_buf(),
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(io("create", path))
        }
        Err(error) => Err(io("inspect", path)(error)),
    }
}

/// Writes `bytes` beside `path` and renames it over: a symbolic link at `path` is replaced
/// itself, its target never written.
fn replace(path: &Path, bytes: &[u8]) -> Result<(), WriteError> {
    let file_name = path
        .file_name()
        .expect("a bundle path names a file")
        .to_string_lossy();
    let temporary = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    fs::write(&temporary, bytes).map_err(io("write", &temporary))?;
    fs::rename(&temporary, path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        io("replace", path)(error)
    })
}

/// Removes the files and links in `directory` this release does not ship. Directories are
/// left alone: the bundle never had any but its own.
fn remove_strays(directory: &Path, expected: &BTreeSet<PathBuf>) -> Result<(), WriteError> {
    for entry in fs::read_dir(directory).map_err(io("read", directory))? {
        let entry = entry.map_err(io("read", directory))?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(io("inspect", &path))?;
        if file_type.is_dir() || expected.contains(&path) {
            continue;
        }
        fs::remove_file(&path).map_err(io("remove", &path))?;
    }
    Ok(())
}

fn io(action: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> WriteError {
    let path = path.to_path_buf();
    move |source| WriteError::Io {
        action,
        path,
        source,
    }
}
