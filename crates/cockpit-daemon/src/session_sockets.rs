//! Opening a session's provider socket only when the session's own process tree listens (#442).
//!
//! The session's directory is the agent's to write (#437), so a socket path there names
//! whatever the agent chose: a link to the owner's unsandboxed app-server, another session's,
//! or any socket the daemon may open. The path is never trusted; the kernel's record of the
//! listening process is. A link into a folder the sandbox mounts from the session's own
//! (Codex's socket folder, #446) is read as that folder's entry, never followed to the host's
//! folder of that name.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cockpit_agent_channel::ProcessIdentity;
use cockpit_provider_api::{ProviderError, SessionSockets};
use cockpit_sandbox::ProviderReach;

/// How long connecting may wait, since `Provider::deliver` blocks for seconds at most. On
/// Linux a listener whose queue is full refuses at once (`EAGAIN`) rather than holding the
/// connect; the bound is for any other wait.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Sockets accepted only when the listening process descends from the session's agent root,
/// the process Cockpit launched for it. A listener the agent started inside its own tree is
/// accepted: it already holds the conversation the daemon delivers to it.
#[derive(Debug, Clone)]
pub struct SessionTreeSockets {
    pub root: ProcessIdentity,
    /// How the launch's sandbox left the provider's sockets reachable: the launch plan's
    /// `reach`, or [`ProviderReach::Direct`] for an agent launched unsandboxed.
    pub reach: ProviderReach,
}

impl SessionSockets for SessionTreeSockets {
    /// Blocking, for [`CONNECT_TIMEOUT`] at most and a read of the kernel's process table.
    /// Nothing is written to the socket before its listener is verified; the connect itself
    /// reaches whatever the path names.
    fn connect(&self, socket: &Path) -> Result<UnixStream, ProviderError> {
        let refused = |reason: String| ProviderError {
            reason: format!("{} was not opened: {reason}", socket.display()),
        };
        let host_socket = self.host_socket(socket).map_err(refused)?;
        let (_folder, reachable) = short_path(&host_socket).map_err(refused)?;
        let (stream, listener) = connect_to_listener(&reachable).map_err(refused)?;
        let chain = cockpit_agent_channel::ancestry(listener).map_err(|error| {
            refused(format!("its listener {listener} is unverifiable: {error}"))
        })?;
        if !chain.contains(&self.root) {
            return Err(refused(format!(
                "its listener {listener} is not in the session's process tree (root {})",
                self.root.pid
            )));
        }
        Ok(stream)
    }
}

impl SessionTreeSockets {
    /// Where the daemon finds `socket` on the host. Read without connecting to anything.
    fn host_socket(&self, socket: &Path) -> Result<PathBuf, String> {
        let mounts = match &self.reach {
            ProviderReach::Direct => return Ok(socket.to_owned()),
            ProviderReach::Unreachable { reason, .. } => return Err(reason.clone()),
            ProviderReach::Mounted(mounts) => mounts,
        };
        let is_link = std::fs::symlink_metadata(socket)
            .map_err(|error| error.to_string())?
            .is_symlink();
        if !is_link {
            return Ok(socket.to_owned());
        }
        let target = std::fs::read_link(socket).map_err(|error| error.to_string())?;
        mounts
            .iter()
            .find_map(|mount| mount.host_path(&target))
            .ok_or_else(|| {
                format!(
                    "it links to {}, outside the folders the sandbox mounts",
                    target.display()
                )
            })
    }
}

/// A path to `socket` that fits a unix socket address (108 bytes on Linux): Codex's entries
/// are 64-character names, too long under the daemon's own folders. On Linux the entry is
/// named through its open folder, `/proc/self/fd/<folder>/<entry>`, valid while the
/// returned folder stays open; elsewhere the path is used as it is.
fn short_path(socket: &Path) -> Result<(Option<std::fs::File>, PathBuf), String> {
    if !cfg!(target_os = "linux") {
        return Ok((None, socket.to_owned()));
    }
    use std::os::fd::AsRawFd;
    let (Some(folder), Some(entry)) = (socket.parent(), socket.file_name()) else {
        return Ok((None, socket.to_owned()));
    };
    let opened = std::fs::File::open(folder)
        .map_err(|error| format!("its folder {} cannot be opened: {error}", folder.display()))?;
    let path = PathBuf::from(format!("/proc/self/fd/{}", opened.as_raw_fd())).join(entry);
    Ok((Some(opened), path))
}

/// A blocking connection to `socket` and the pid the kernel recorded for its listener.
/// std's `peer_cred` is unstable and its connect has no timeout, so tokio's serve both, on a
/// thread and runtime of their own: a caller's current-thread runtime could not drive the
/// connect from a blocking call. The stream is handed back blocking.
fn connect_to_listener(socket: &Path) -> Result<(UnixStream, u32), String> {
    std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()
                    .map_err(|error| format!("cannot connect: {error}"))?;
                runtime.block_on(connect_within_timeout(socket))
            })
            .join()
            .map_err(|_| "connecting panicked".to_owned())?
    })
}

async fn connect_within_timeout(socket: &Path) -> Result<(UnixStream, u32), String> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::UnixStream::connect(socket))
        .await
        .map_err(|_| format!("its listener did not answer within {CONNECT_TIMEOUT:?}"))?
        .map_err(|error| error.to_string())?;
    let unreadable = |error: std::io::Error| format!("cannot read its peer: {error}");
    let pid = stream
        .peer_cred()
        .map_err(unreadable)?
        .pid()
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0)
        .ok_or_else(|| "the kernel named no listening process".to_owned())?;
    let stream = stream.into_std().map_err(unreadable)?;
    stream.set_nonblocking(false).map_err(unreadable)?;
    Ok((stream, pid))
}
