//! Host-service forwards (#880): the launcher serves each declared service's listener and
//! relays a connection's bytes to the service's target on the host, unparsed, only while
//! the owner's decisions allow the service's record. An ungranted connection is closed at
//! once and its record listed in the requests file, which asks the owner; a revoked decision
//! closes the relays it allowed. The session proxy's own refusals (#831) are untouched: a
//! forward is never a `CONNECT`.

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cockpit_sandbox::{AskedService, EgressCounters, Endpoint, EngineRelay, Forward};

use rustix::net::sockopt::{Timeout, set_socket_timeout};
use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType, connect, socket_with};

use super::{CONNECT_TIMEOUT, Proxy, Upstream};
use crate::plan::SocketFolder;

/// The folder of the session's mount folder holding the socket forwards' listeners, one
/// folder inside per `listen` folder.
const SOCKETS_FOLDER: &str = "forward-sockets";

/// Socket paths longer than this are reached through their folder's descriptor: the kernel
/// limits a socket's path to 107 bytes.
const MAX_SOCKET_PATH: usize = 100;

/// A forward's listener: a port inside the sandbox, handed out by stage 2, or a socket the
/// launcher created in a folder of its own.
pub enum Listener {
    Port(TcpListener),
    Socket(UnixListener),
}

impl Listener {
    fn accept(&self) -> std::io::Result<Stream> {
        match self {
            Listener::Port(listener) => listener.accept().map(|(stream, _)| Stream::Tcp(stream)),
            Listener::Socket(listener) => listener.accept().map(|(stream, _)| Stream::Unix(stream)),
        }
    }
}

/// One end of a relay.
pub enum Stream {
    Tcp(TcpStream),
    Unix(UnixStream),
}

impl Stream {
    fn try_clone(&self) -> std::io::Result<Stream> {
        match self {
            Stream::Tcp(stream) => stream.try_clone().map(Stream::Tcp),
            Stream::Unix(stream) => stream.try_clone().map(Stream::Unix),
        }
    }

    fn shutdown(&self, how: Shutdown) {
        let _ = match self {
            Stream::Tcp(stream) => stream.shutdown(how),
            Stream::Unix(stream) => stream.shutdown(how),
        };
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Stream::Tcp(stream) => stream.read(buffer),
            Stream::Unix(stream) => stream.read(buffer),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match self {
            Stream::Tcp(stream) => stream.write(bytes),
            Stream::Unix(stream) => stream.write(bytes),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Stream::Tcp(stream) => stream.flush(),
            Stream::Unix(stream) => stream.flush(),
        }
    }
}

/// One open relay: the forward that allowed it and both ends, to close on a revocation.
pub(super) struct OpenRelay {
    service: String,
    record: String,
    client: Stream,
    target: Stream,
}

/// The socket forwards' listeners, and the folders that hold them.
pub struct BoundSockets {
    /// To bind read-only at the `listen` paths' folders.
    pub folders: Vec<SocketFolder>,
    /// Each with the index of its forward in the spec, in the spec's order.
    pub listeners: Vec<(usize, UnixListener)>,
}

/// The listeners of `forwards`' socket listens, created in fresh folders of `mount_dir`
/// before the sandbox starts. The program connects to them; it can replace neither the
/// folder nor the socket.
pub fn bind_sockets(mount_dir: &Path, forwards: &[Forward]) -> Result<BoundSockets, String> {
    let root = mount_dir.join(SOCKETS_FOLDER);
    match std::fs::remove_dir_all(&root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot remove {}: {error}", root.display())),
    }
    let mut folders: Vec<SocketFolder> = Vec::new();
    let mut listeners = Vec::new();
    for (index, forward) in forwards.iter().enumerate() {
        let Endpoint::Socket { path } = &forward.listen else {
            continue;
        };
        let (Some(destination), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(format!(
                "service {} listens on {}, which has no folder",
                forward.service,
                path.display()
            ));
        };
        let source = match folders
            .iter()
            .find(|folder| folder.destination == destination)
        {
            Some(folder) => folder.source.clone(),
            None => {
                let source = root.join(folders.len().to_string());
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(&source)
                    .map_err(|error| format!("cannot create {}: {error}", source.display()))?;
                folders.push(SocketFolder {
                    source: source.clone(),
                    destination: destination.to_path_buf(),
                });
                source
            }
        };
        let listener =
            crate::handoff::in_folder(&source, name, UnixListener::bind).map_err(|error| {
                format!(
                    "service {} cannot listen on {}: {error}",
                    forward.service,
                    path.display()
                )
            })?;
        listeners.push((index, listener));
    }
    Ok(BoundSockets { folders, listeners })
}

impl<U: Upstream> Proxy<U> {
    /// Serves `forward` on `listener` until the process ends: one thread per connection.
    pub fn serve_forward(self: &Arc<Self>, forward: Forward, listener: Listener) {
        self.serve_forward_until(forward, &listener, &|| false);
    }

    /// Serves `forward` on `listener` until `is_closed` holds once its accept fails.
    pub(super) fn serve_forward_until(
        self: &Arc<Self>,
        forward: Forward,
        listener: &Listener,
        is_closed: &dyn Fn() -> bool,
    ) {
        // Listed in the evidence from the start, connected or not.
        self.count_forward(&forward.service, |_| {});
        let forward = Arc::new(forward);
        loop {
            let Ok(client) = listener.accept() else {
                if is_closed() {
                    return;
                }
                std::thread::sleep(super::ACCEPT_RETRY);
                continue;
            };
            let proxy = Arc::clone(self);
            let serving = Arc::clone(&forward);
            let spawned = std::thread::Builder::new()
                .name("forward".to_owned())
                .spawn(move || proxy.relay(&serving, client));
            // Out of threads: the client's connection closes, which it sees at once.
            drop(spawned);
        }
    }

    /// Relays one connection to `forward`'s target, or closes it at once saying why in the
    /// counters.
    fn relay(&self, forward: &Forward, client: Stream) {
        let decided = self.decided();
        let services = &decided.decisions.services;
        let Some(allowed) = services.allowing(forward) else {
            let reason = match services.denial(forward) {
                Some(request) => format!("the owner denied service request {request}"),
                // Opened by a decision, so asked of the owner already: its listener closes.
                None if forward.live_problem().is_none() => {
                    format!("the owner's decisions no longer allow {}", forward.record())
                }
                None => self.ask_service(forward),
            };
            self.refuse_forward(forward, &client, reason);
            return;
        };
        let source = allowed.source.to_string();
        let target = match self.connect_target(&forward.target, services.engine.as_ref()) {
            Ok(target) => target,
            Err(error) => {
                client.shutdown(Shutdown::Both);
                self.count_forward(&forward.service, |counters| {
                    counters.unreachable += 1;
                    counters.last_reason =
                        Some(format!("{} is unreachable: {error}", forward.target));
                });
                return;
            }
        };
        self.count_forward(&forward.service, |counters| {
            counters.allowed += 1;
            counters.source = Some(source);
        });
        let Some(relay) = self.open_relay(forward, &client, &target) else {
            return;
        };
        self.splice_forward(&forward.service, client, target);
        self.relays
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&relay);
    }

    fn refuse_forward(&self, forward: &Forward, client: &Stream, reason: String) {
        client.shutdown(Shutdown::Both);
        self.count_forward(&forward.service, |counters| {
            counters.refused += 1;
            counters.last_reason = Some(reason);
        });
    }

    /// Lists `forward`'s record in the requests file, and says what the owner is asked.
    fn ask_service(&self, forward: &Forward) -> String {
        if self.requests_file.get().is_none() {
            return format!(
                "service {} is not granted, and this launch asks nobody",
                forward.service
            );
        }
        let mut asked = self
            .asked
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let service = AskedService::of(forward);
        let decided = self.decided();
        if !asked.services.contains(&service) && !decided.decisions.services.covers(forward) {
            asked.services.insert(service.clone());
            if let Err(error) = self.write_requests(&mut asked) {
                asked.services.remove(&service);
                return format!(
                    "service {} is not granted; the owner could not be asked: {error}",
                    forward.service
                );
            }
        }
        match decided.decisions.services.asking(forward) {
            Some(request) => format!(
                "service {} is not granted; the owner is asked in service request {request}",
                forward.service
            ),
            None => format!(
                "service {} is not granted; the owner is asked",
                forward.service
            ),
        }
    }

    fn connect_target(
        &self,
        target: &Endpoint,
        engine: Option<&EngineRelay>,
    ) -> std::io::Result<Stream> {
        match target {
            Endpoint::Port { port } => self
                .upstream
                .connect(SocketAddr::from(([127, 0, 0, 1], *port)), CONNECT_TIMEOUT)
                .map(Stream::Tcp),
            Endpoint::Socket { path } => connect_socket(path).map(Stream::Unix),
            Endpoint::Engine { user, port } => {
                let relay = engine.filter(|relay| &relay.user == user).ok_or_else(|| {
                    std::io::Error::other(format!("the owner recorded no engine run by {user}"))
                })?;
                let stream = connect_socket(&relay.socket)?;
                crate::engine_relay::request(stream, relay.uid, *port).map(Stream::Unix)
            }
        }
    }

    /// Registers the relay `forward` allowed, so a revocation closes it; None, closing both
    /// ends, when the decision was taken back since the connection was decided.
    fn open_relay(&self, forward: &Forward, client: &Stream, target: &Stream) -> Option<u64> {
        let (Ok(client_end), Ok(target_end)) = (client.try_clone(), target.try_clone()) else {
            return None;
        };
        let number = self.next_tunnel.fetch_add(1, Ordering::Relaxed);
        let mut relays = self
            .relays
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Checked under the lock `apply_decisions` closes relays under: a revocation is either
        // seen here or closes this relay there.
        if self
            .decided()
            .decisions
            .services
            .allowing(forward)
            .is_none()
        {
            client.shutdown(Shutdown::Both);
            target.shutdown(Shutdown::Both);
            return None;
        }
        relays.insert(
            number,
            OpenRelay {
                service: forward.service.clone(),
                record: forward.record(),
                client: client_end,
                target: target_end,
            },
        );
        Some(number)
    }

    /// Closes every open relay whose record `allowed` no longer holds.
    pub(super) fn close_revoked_relays(&self, allowed: &cockpit_sandbox::ServiceDecisions) {
        let relays = self
            .relays
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for relay in relays.values() {
            if allowed
                .allowing_record(&relay.service, &relay.record)
                .is_none()
            {
                relay.client.shutdown(Shutdown::Both);
                relay.target.shutdown(Shutdown::Both);
            }
        }
    }

    /// Copies both ways until the target ends, or each side does, counting the bytes.
    fn splice_forward(&self, service: &str, client: Stream, target: Stream) {
        let (Ok(client_reader), Ok(target_reader)) = (client.try_clone(), target.try_clone())
        else {
            return;
        };
        std::thread::scope(|scope| {
            scope.spawn(|| self.copy_forward(service, client_reader, target, true));
            self.copy_forward(service, target_reader, client, false);
        });
    }

    fn copy_forward(&self, service: &str, mut from: Stream, mut to: Stream, is_out: bool) {
        let mut buffer = vec![0_u8; 16 * 1024];
        loop {
            let read = match from.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            if to.write_all(&buffer[..read]).is_err() {
                break;
            }
            let bytes = read as u64;
            self.count_forward(service, |counters| {
                if is_out {
                    counters.bytes_out += bytes;
                } else {
                    counters.bytes_in += bytes;
                }
            });
        }
        if is_out {
            // A client may half-close and still read the answer.
            to.shutdown(Shutdown::Write);
        } else {
            // Once the target ended, a client left open would hold its thread for good.
            to.shutdown(Shutdown::Both);
            from.shutdown(Shutdown::Both);
        }
    }

    pub(super) fn count_forward(&self, service: &str, update: impl FnOnce(&mut EgressCounters)) {
        let mut report = self
            .report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update(report.forwards.entry(service.to_owned()).or_default());
        self.changes.fetch_add(1, Ordering::Relaxed);
    }
}

/// Connects to the socket at `path`, through its folder's descriptor when the path is too
/// long for the kernel.
fn connect_socket(path: &Path) -> std::io::Result<UnixStream> {
    if path.as_os_str().len() <= MAX_SOCKET_PATH {
        return connect_socket_within(path, CONNECT_TIMEOUT);
    }
    let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(std::io::Error::other("the socket path has no folder"));
    };
    crate::handoff::in_folder(folder, name, |reached: PathBuf| {
        connect_socket_within(&reached, CONNECT_TIMEOUT)
    })
}

/// Connects within `timeout`, as a port target does: a Unix socket's connect waits for room
/// in a full backlog for as long as the socket's send timeout, which is unset by default.
fn connect_socket_within(path: &Path, timeout: Duration) -> std::io::Result<UnixStream> {
    let socket = socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )?;
    set_socket_timeout(&socket, Timeout::Send, Some(timeout))?;
    connect(&socket, &SocketAddrUnix::new(path)?)?;
    // The relay's writes wait on the client, not on a timeout.
    set_socket_timeout(&socket, Timeout::Send, None)?;
    Ok(UnixStream::from(socket))
}

#[cfg(test)]
mod tests;
