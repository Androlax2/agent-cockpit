//! One daemon per OS user: the private socket directory and the instance lock (report 11).

use std::fs::{self, DirBuilder, File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::StartError;
use crate::server::Limits;

const SOCKET_FILE: &str = "daemon.sock";
/// The agent socket has a directory of its own, so a sandbox can be given that directory
/// without the host-only socket beside it.
const AGENT_SOCKET_DIR: &str = "agent";
const AGENT_SOCKET_FILE: &str = "agent.sock";
const LOCK_FILE: &str = "daemon.lock";
const DATABASE_FILE: &str = "state.db";
/// Only the owner may enter the socket directory.
const PRIVATE_DIR_MODE: u32 = 0o700;

/// Where one daemon keeps its files, and how much work it accepts.
#[derive(Debug, Clone)]
pub struct Options {
    /// `$XDG_RUNTIME_DIR/cockpit`: the socket and the instance lock.
    pub runtime_dir: PathBuf,
    /// `$XDG_STATE_HOME/cockpit`: the state database and the logs.
    pub state_dir: PathBuf,
    pub limits: Limits,
}

impl Options {
    /// Reads the XDG layout. Without `XDG_RUNTIME_DIR` (macOS, some containers) the socket
    /// directory is `<state dir>/run`: the XDG spec leaves that fallback to the program, and
    /// the instance lock, not the directory's lifetime, tells a stale socket apart.
    pub fn from_environment(
        environment: &cockpit_config::Environment,
    ) -> Result<Options, StartError> {
        let home = environment.home_dir().map_err(StartError::Config)?;
        let layout = cockpit_config::Layout::resolve(environment, &home);
        let state_dir = layout.state_dir.value;
        if let cockpit_config::Source::InvalidEnvironment(variable) = layout.runtime_dir.source {
            return Err(StartError::PrivateDir {
                path: PathBuf::from(format!("${variable}")),
                reason: "not an absolute path".to_owned(),
            });
        }
        let runtime_dir = layout
            .runtime_dir
            .value
            .unwrap_or_else(|| state_dir.join("run"));
        Ok(Options {
            runtime_dir,
            state_dir,
            limits: Limits::default(),
        })
    }

    /// The host-only socket: its peers are the owner. No sandbox may reach it.
    pub fn socket_path(&self) -> PathBuf {
        self.runtime_dir.join(SOCKET_FILE)
    }

    /// The socket sandboxed agents reach; its peers are never the owner.
    pub fn agent_socket_path(&self) -> PathBuf {
        self.runtime_dir
            .join(AGENT_SOCKET_DIR)
            .join(AGENT_SOCKET_FILE)
    }

    pub(crate) fn database_path(&self) -> PathBuf {
        self.state_dir.join(DATABASE_FILE)
    }
}

/// This user's only daemon: it holds the instance lock until dropped.
#[derive(Debug)]
pub struct Instance {
    pub(crate) options: Options,
    /// The daemon's user id; only peers with the same id are served.
    pub(crate) owner_uid: u32,
    _lock: File,
}

impl Instance {
    /// Creates the private directories and takes the instance lock. A second daemon of the
    /// same user fails here with [`StartError::AlreadyRunning`], before it touches the
    /// socket, the logs or the database.
    pub fn acquire(options: Options) -> Result<Instance, StartError> {
        options.limits.validate()?;
        let owner_uid = effective_uid(&options.state_dir)?;
        create_private_dir(&options.state_dir, owner_uid)?;
        create_private_dir(&options.runtime_dir, owner_uid)?;
        create_private_dir(&options.runtime_dir.join(AGENT_SOCKET_DIR), owner_uid)?;
        let lock_path = options.runtime_dir.join(LOCK_FILE);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(StartError::io("open the instance lock", &lock_path))?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(StartError::AlreadyRunning { lock_path }),
            Err(TryLockError::Error(source)) => {
                return Err(StartError::io("lock", &lock_path)(source));
            }
        }
        Ok(Instance {
            options,
            owner_uid,
            _lock: lock,
        })
    }

    pub fn options(&self) -> &Options {
        &self.options
    }
}

/// The user id this process creates files as, read from a file it creates next to
/// `near` (its parent, created if missing), since `geteuid` would need `unsafe`.
fn effective_uid(near: &Path) -> Result<u32, StartError> {
    let parent = near.parent().unwrap_or(near);
    fs::create_dir_all(parent).map_err(StartError::io("create", parent))?;
    let probe = parent.join(format!(".cockpit-daemon-uid-{}", std::process::id()));
    let created = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .and_then(|file| file.metadata());
    let _ = fs::remove_file(&probe);
    Ok(created.map_err(StartError::io("create", &probe))?.uid())
}

/// Creates `path` (and missing parents) 0700, or checks that the existing directory is a
/// real directory of `owner_uid` nobody else may open. A wider mode is refused, never
/// tightened: someone else may already have reached it.
fn create_private_dir(path: &Path, owner_uid: u32) -> Result<(), StartError> {
    match DirBuilder::new()
        .recursive(true)
        .mode(PRIVATE_DIR_MODE)
        .create(path)
    {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
        Err(source) => return Err(StartError::io("create", path)(source)),
    }
    let metadata = fs::symlink_metadata(path).map_err(StartError::io("inspect", path))?;
    let refuse = |reason: String| {
        Err(StartError::PrivateDir {
            path: path.to_path_buf(),
            reason,
        })
    };
    if !metadata.file_type().is_dir() {
        return refuse("is not a directory (a symbolic link is refused)".to_owned());
    }
    if metadata.uid() != owner_uid {
        return refuse(format!(
            "belongs to user id {}, not to this daemon's user id {owner_uid}",
            metadata.uid()
        ));
    }
    let mode = metadata.permissions().mode() & 0o777;
    if mode != PRIVATE_DIR_MODE {
        return refuse(format!(
            "has mode {mode:03o}; it must be {PRIVATE_DIR_MODE:03o} (run `chmod 700` on it)"
        ));
    }
    Ok(())
}
