//! One daemon per OS user: the private socket directory and the instance lock (report 11).

use std::fs::{self, DirBuilder, File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::StartError;
use crate::server::Limits;

const SOCKET_FILE: &str = "daemon.sock";
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

    pub fn socket_path(&self) -> PathBuf {
        self.runtime_dir.join(SOCKET_FILE)
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
        create_private_dir(&options.state_dir)?;
        create_private_dir(&options.runtime_dir)?;
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
        // The directory is 0700 and this process could create a file in it, so it is this
        // user's: its owner is the daemon's user id, read without unsafe `geteuid`.
        let owner_uid = fs::metadata(&options.runtime_dir)
            .map_err(StartError::io("inspect", &options.runtime_dir))?
            .uid();
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

/// Creates `path` (and missing parents) 0700, or checks that the existing directory is a
/// real directory nobody else may open. A wider mode is refused, never tightened: someone
/// else may already have reached it.
fn create_private_dir(path: &Path) -> Result<(), StartError> {
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
    let mode = metadata.permissions().mode() & 0o777;
    if mode != PRIVATE_DIR_MODE {
        return refuse(format!(
            "has mode {mode:03o}; it must be {PRIVATE_DIR_MODE:03o} (run `chmod 700` on it)"
        ));
    }
    Ok(())
}
