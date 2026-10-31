//! Per-project answers directories: where the host writes what it answers a project's agents
//! (run output, guide answers). Only host programs write them; the sandbox launcher mounts a
//! project's own directory, read-only, into that project's sandboxes and no other, so no
//! session reads another project's answers (2026-10-19 security audit, M3).

use std::fmt;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const PRIVATE_DIR_MODE: u32 = 0o700;
/// Answers are never rewritten in place: a sandbox that reaches one through a misconfigured
/// writable mount still finds it read-only.
const ANSWER_MODE: u32 = 0o400;
const MAX_NAME_BYTES: usize = 255;

/// A project's answers directory name: ASCII letters, digits and `-`, so it never leaves
/// the answers root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectKey(String);

impl ProjectKey {
    pub fn new(key: impl Into<String>) -> Result<ProjectKey, AnswersError> {
        let key = key.into();
        let is_valid = !key.is_empty()
            && key.len() <= MAX_NAME_BYTES
            && key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
        if !is_valid {
            return Err(AnswersError::InvalidName {
                name: key,
                rule: "a project key is 1 to 255 ASCII letters, digits or '-'",
            });
        }
        Ok(ProjectKey(key))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One project's answers directory, checked private to its owner.
#[derive(Debug, Clone)]
pub struct AnswersDirectory {
    path: PathBuf,
}

impl AnswersDirectory {
    /// Creates `<answers_root>/<project>` (and the root) 0700, or checks the existing ones.
    /// The root must belong to the owner of its parent, which the daemon keeps private; a
    /// symbolic link, another owner or a wider mode is refused, never repaired.
    pub fn prepare(answers_root: &Path, project: &ProjectKey) -> Result<Self, AnswersError> {
        let parent = answers_root.parent().ok_or_else(|| AnswersError::Unsafe {
            path: answers_root.to_path_buf(),
            reason: "has no parent directory".to_owned(),
        })?;
        let owner_uid = fs::metadata(parent)
            .map_err(AnswersError::io("inspect", parent))?
            .uid();
        create_private_dir(answers_root, owner_uid)?;
        let path = answers_root.join(project.as_str());
        create_private_dir(&path, owner_uid)?;
        Ok(AnswersDirectory { path })
    }

    /// The directory the launcher mounts read-only into the project's sandboxes.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Writes one answer as `name`, whole or not at all: readers never see a partial file.
    /// An answer is written once; a second answer with the same name is refused.
    pub fn write(&self, name: &str, contents: &[u8]) -> Result<PathBuf, AnswersError> {
        let is_valid = !name.is_empty()
            && name.len() <= MAX_NAME_BYTES
            && !name.starts_with('.')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte));
        if !is_valid {
            return Err(AnswersError::InvalidName {
                name: name.to_owned(),
                rule: "an answer name is ASCII letters, digits, '-', '_' or '.', not starting with '.'",
            });
        }
        let destination = self.path.join(name);
        let partial = self.path.join(partial_name(name));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(ANSWER_MODE)
            .open(&partial)
            .map_err(AnswersError::io("create", &partial))?;
        // A hard link, unlike a rename, never replaces an existing file: of two writers of
        // one name, the second is refused instead of overwriting the first.
        let written = file
            .write_all(contents)
            .and_then(|()| file.sync_all())
            .map_err(AnswersError::io("write", &partial))
            .and_then(|()| match fs::hard_link(&partial, &destination) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    Err(AnswersError::AlreadyAnswered {
                        path: destination.clone(),
                    })
                }
                Err(error) => Err(AnswersError::io("publish", &destination)(error)),
            });
        // Best effort: the outcome is already decided, and a leftover partial is a dot file
        // under a name no answer can take, so no reader mistakes it for an answer.
        let _ = fs::remove_file(&partial);
        written.map(|()| destination)
    }
}

/// A partial file's name, unique to this write: the pid alone would collide between two
/// threads, and with a crashed writer's leftover once the pid is reused.
fn partial_name(name: &str) -> String {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let write = WRITES.fetch_add(1, Ordering::Relaxed);
    format!(".{name}.partial-{}-{nanos}-{write}", std::process::id())
}

fn create_private_dir(path: &Path, owner_uid: u32) -> Result<(), AnswersError> {
    match DirBuilder::new().mode(PRIVATE_DIR_MODE).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(error) => return Err(AnswersError::io("create", path)(error)),
    }
    let metadata = fs::symlink_metadata(path).map_err(AnswersError::io("inspect", path))?;
    let unsafe_dir = |reason: String| {
        Err(AnswersError::Unsafe {
            path: path.to_path_buf(),
            reason,
        })
    };
    if !metadata.file_type().is_dir() {
        return unsafe_dir("is not a directory (a symbolic link is refused)".to_owned());
    }
    if metadata.uid() != owner_uid {
        return unsafe_dir(format!(
            "belongs to user id {}, not {owner_uid}",
            metadata.uid()
        ));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode != PRIVATE_DIR_MODE {
        return unsafe_dir(format!("has mode {mode:03o}; it must be 700"));
    }
    Ok(())
}

#[derive(Debug)]
pub enum AnswersError {
    InvalidName {
        name: String,
        rule: &'static str,
    },
    /// A directory on the path is not private to its owner.
    Unsafe {
        path: PathBuf,
        reason: String,
    },
    AlreadyAnswered {
        path: PathBuf,
    },
    Io {
        action: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
}

impl AnswersError {
    fn io(action: &'static str, path: &Path) -> impl FnOnce(std::io::Error) -> AnswersError {
        let path = path.to_path_buf();
        move |source| AnswersError::Io {
            action,
            path,
            source,
        }
    }
}

impl fmt::Display for AnswersError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnswersError::InvalidName { name, rule } => write!(formatter, "{name:?}: {rule}"),
            AnswersError::Unsafe { path, reason } => {
                write!(formatter, "{}: {reason}", path.display())
            }
            AnswersError::AlreadyAnswered { path } => {
                write!(formatter, "{} is already answered", path.display())
            }
            AnswersError::Io {
                action,
                path,
                source,
            } => write!(formatter, "cannot {action} {}: {source}", path.display()),
        }
    }
}

impl std::error::Error for AnswersError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AnswersError::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
