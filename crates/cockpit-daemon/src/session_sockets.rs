//! Opening a session's provider socket only when the session's own process tree listens (#442).
//!
//! The session's directory is the agent's to write (#437), so a socket path there names
//! whatever the agent chose: a link to the owner's unsandboxed app-server, another session's,
//! or any socket the daemon may open. The path is never trusted; the kernel's record of the
//! listening process is.

use std::os::unix::net::UnixStream;
use std::path::Path;

use cockpit_agent_channel::ProcessIdentity;
use cockpit_provider_api::{ProviderError, SessionSockets};

/// Sockets accepted only when the listening process descends from the session's agent root,
/// the process Cockpit launched for it. A listener the agent started inside its own tree is
/// accepted: it already holds the conversation the daemon delivers to it.
#[derive(Debug, Clone)]
pub struct SessionTreeSockets {
    pub root: ProcessIdentity,
}

impl SessionSockets for SessionTreeSockets {
    /// Blocking: it reads the kernel's process table. Nothing is written to the socket
    /// before its listener is verified.
    fn connect(&self, socket: &Path) -> Result<UnixStream, ProviderError> {
        let refused = |reason: String| ProviderError {
            reason: format!("{} was not opened: {reason}", socket.display()),
        };
        let stream = UnixStream::connect(socket).map_err(|error| refused(error.to_string()))?;
        let (stream, listener) = peer_pid(stream).map_err(refused)?;
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

/// The pid the kernel recorded for the other end. std's `peer_cred` is unstable, so tokio's
/// reads it; the stream is handed back blocking, as it came.
fn peer_pid(stream: UnixStream) -> Result<(UnixStream, u32), String> {
    let own_runtime;
    let handle = match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle,
        Err(_) => {
            own_runtime = tokio::runtime::Builder::new_current_thread()
                .enable_io()
                .build()
                .map_err(|error| format!("cannot read its peer: {error}"))?;
            own_runtime.handle().clone()
        }
    };
    let _entered = handle.enter();
    let unreadable = |error: std::io::Error| format!("cannot read its peer: {error}");
    stream.set_nonblocking(true).map_err(unreadable)?;
    let stream = tokio::net::UnixStream::from_std(stream).map_err(unreadable)?;
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
