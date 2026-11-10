//! The proxy listener's way out of the sandbox (#546 M4 A): stage 2 creates it on the
//! loopback of the sandbox's empty network namespace, then passes it to the launcher, which
//! serves it from the host's network. The agent connects to an address inside its own
//! namespace; nothing of the host's network is in there. Each port forward's listener
//! (#880) travels with it, in the spec's order.
//!
//! The hand-off goes through a named Unix socket in a folder of the launcher's own, bound
//! read-only into the view and never granted by Landlock: stage 2 connects before the
//! program runs, and the launcher removes the socket as soon as it has the listener.
//!
//! The connection stays open for the binder (#390): a process stage 2 starts beside the
//! program, under the same Landlock and seccomp, which binds one more loopback listener
//! whenever the launcher asks and sends it over. It binds only the port asked, the launcher
//! checks the address of each listener it receives, and the launcher alone decides what a
//! listener reaches. A binder that is gone opens nothing more.

use std::fs::File;
use std::io::{IoSlice, IoSliceMut, Read, Write};
use std::mem::MaybeUninit;
use std::net::{SocketAddr, TcpListener};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::{Duration, Instant};

use cockpit_sandbox::{MAX_FORWARDS, PROXY_ADDRESS};
use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags, recvmsg, sendmsg,
};

const SOCKET_NAME: &str = "handoff.sock";

/// How long the launcher waits for stage 2's listener while bubblewrap runs, and stage 2
/// for the launcher's answer.
const HANDOFF_TIMEOUT: Duration = Duration::from_secs(30);

/// What stage 2 sends with the listener, and the launcher answers once it holds it.
const LISTENER_BYTE: u8 = b'L';
const RECEIVED_BYTE: u8 = b'R';

/// The launcher asks the binder for a listener with this byte and the port, big-endian; the
/// binder answers [`LISTENER_BYTE`] with the listener, or this byte, a length and why not.
const BIND_BYTE: u8 = b'B';
const REFUSED_BYTE: u8 = b'E';

/// The verb stage 2 starts the binder with. Never run by hand.
pub const BINDER_VERB: &str = "stage2-binder";

/// The longest reason a binder sends.
const MAX_REASON: usize = 512;

/// The launcher's side: the hand-off socket, listening until stage 2 connects. Dropping it
/// removes its folder.
pub struct Waiting {
    folder: PathBuf,
    listener: UnixListener,
}

impl Waiting {
    /// Creates `folder` afresh, owner-only, with the hand-off socket inside.
    pub fn open(folder: PathBuf) -> Result<Waiting, String> {
        match std::fs::remove_dir_all(&folder) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!("cannot remove {}: {error}", folder.display()));
            }
        }
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&folder)
            .map_err(|error| format!("cannot create {}: {error}", folder.display()))?;
        let socket = folder.join(SOCKET_NAME);
        let listener = in_folder(&folder, SOCKET_NAME, UnixListener::bind)
            .map_err(|error| format!("cannot listen on {}: {error}", socket.display()))?;
        Ok(Waiting { folder, listener })
    }

    pub fn socket(&self) -> PathBuf {
        self.folder.join(SOCKET_NAME)
    }

    /// Stage 2's proxy listener and one listener per port of `forward_ports`, in their order,
    /// once it sent them; None when `child` ended first, so the launcher reports the child's
    /// own failure.
    pub fn receive(
        self,
        child: &mut Child,
        forward_ports: &[u16],
    ) -> Result<Option<Received>, String> {
        self.listener
            .set_nonblocking(true)
            .map_err(|error| format!("cannot wait for the proxy listener: {error}"))?;
        let deadline = Instant::now() + HANDOFF_TIMEOUT;
        let stream = loop {
            match self.listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(format!("cannot take the proxy listener: {error}")),
            }
            let ended = child
                .try_wait()
                .map_err(|error| format!("cannot watch bubblewrap: {error}"))?;
            if ended.is_some() {
                return Ok(None);
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "stage 2 sent no proxy listener within {}s",
                    HANDOFF_TIMEOUT.as_secs()
                ));
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        take_listeners(stream, forward_ports).map(Some)
    }
}

/// What stage 2 handed over.
pub struct Received {
    pub proxy: TcpListener,
    /// One per forward port, in the order the launcher asked for them.
    pub forwards: Vec<TcpListener>,
    /// Binds the listeners the owner's decisions open after the launch.
    pub binder: Binder,
}

/// The launcher's end of the binder's connection.
pub struct Binder {
    stream: std::sync::Mutex<UnixStream>,
}

impl Binder {
    /// A listener on the sandbox's `127.0.0.1:port`, bound by the binder; why not when the
    /// port is taken or the binder is gone.
    pub fn listen(&self, port: u16) -> Result<TcpListener, String> {
        let stream = self
            .stream
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let failed = |error: std::io::Error| format!("the sandbox's binder is gone: {error}");
        let [high, low] = port.to_be_bytes();
        (&*stream)
            .write_all(&[BIND_BYTE, high, low])
            .map_err(failed)?;
        let (byte, mut descriptors) = receive(&stream, 1).map_err(failed)?;
        match (byte, descriptors.pop()) {
            (LISTENER_BYTE, Some(descriptor)) if descriptors.is_empty() => {
                let listener = TcpListener::from(descriptor);
                check_address(&listener, forward_address(port))?;
                Ok(listener)
            }
            (REFUSED_BYTE, None) => {
                let mut length = [0_u8; 2];
                (&*stream).read_exact(&mut length).map_err(failed)?;
                let mut reason =
                    vec![0_u8; usize::from(u16::from_be_bytes(length)).min(MAX_REASON)];
                (&*stream).read_exact(&mut reason).map_err(failed)?;
                Err(String::from_utf8_lossy(&reason).into_owned())
            }
            _ => Err(format!("the sandbox's binder answered {byte:?}")),
        }
    }
}

/// One byte and the descriptors that came with it, at most `most`.
fn receive(stream: &UnixStream, most: usize) -> std::io::Result<(u8, Vec<std::os::fd::OwnedFd>)> {
    let mut byte = [0_u8; 1];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1 + MAX_FORWARDS))];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    let message = recvmsg(
        stream,
        &mut [IoSliceMut::new(&mut byte)],
        &mut control,
        RecvFlags::CMSG_CLOEXEC,
    )?;
    if message.bytes == 0 {
        return Err(std::io::ErrorKind::UnexpectedEof.into());
    }
    let mut received = Vec::new();
    for message in control.drain() {
        if let RecvAncillaryMessage::ScmRights(descriptors) = message {
            received.extend(descriptors);
        }
    }
    if received.len() > most {
        return Err(std::io::Error::other(format!(
            "{} descriptors came, not at most {most}",
            received.len()
        )));
    }
    Ok((byte[0], received))
}

impl Drop for Waiting {
    fn drop(&mut self) {
        // Nothing else lives there; a folder left behind is replaced at the next launch.
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn take_listeners(mut stream: UnixStream, forward_ports: &[u16]) -> Result<Received, String> {
    let failed = |error: std::io::Error| format!("cannot take the proxy listener: {error}");
    stream.set_nonblocking(false).map_err(failed)?;
    stream
        .set_read_timeout(Some(HANDOFF_TIMEOUT))
        .map_err(failed)?;
    let (byte, received) = receive(&stream, 1 + forward_ports.len()).map_err(failed)?;
    if byte != LISTENER_BYTE {
        return Err(format!(
            "stage 2 sent {byte:?} with its listeners, not {LISTENER_BYTE:?}"
        ));
    }
    if received.len() != 1 + forward_ports.len() {
        return Err(format!(
            "stage 2 sent {} listeners, not the proxy's and {} forwards'",
            received.len(),
            forward_ports.len()
        ));
    }
    let mut listeners = received.into_iter().map(TcpListener::from);
    let proxy = listeners.next().expect("counted above");
    let expected: SocketAddr = PROXY_ADDRESS
        .parse()
        .expect("the proxy address is a socket address");
    check_address(&proxy, expected)?;
    let forwards: Vec<TcpListener> = listeners.collect();
    for (listener, port) in forwards.iter().zip(forward_ports) {
        check_address(listener, forward_address(*port))?;
    }
    stream.write_all(&[RECEIVED_BYTE]).map_err(failed)?;
    stream.set_read_timeout(None).map_err(failed)?;
    Ok(Received {
        proxy,
        forwards,
        binder: Binder {
            stream: std::sync::Mutex::new(stream),
        },
    })
}

/// A listener on `expected`, which stage 2 bound inside the sandbox; one on another address
/// is refused.
fn check_address(listener: &TcpListener, expected: SocketAddr) -> Result<(), String> {
    match listener.local_addr() {
        Ok(address) if address == expected => Ok(()),
        other => Err(format!(
            "stage 2 sent a listener on {other:?}, not on {expected}"
        )),
    }
}

/// Where a port forward listens inside the sandbox: its own loopback.
fn forward_address(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

/// One port forward stage 2 listens for, as its policy names it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ForwardPort {
    pub service: String,
    pub port: u16,
}

/// Stage 2's side, before Landlock and before the program: listens on the proxy address of
/// the sandbox's loopback and on each of `forward_ports`, and sends the listeners to the
/// launcher on `socket`. A port taken already refuses the launch naming its service. Stage 2
/// keeps no copy: the program never holds them. Returns the connection, which the binder
/// serves.
pub fn hand_over(socket: &Path, forward_ports: &[ForwardPort]) -> Result<UnixStream, String> {
    let listener = TcpListener::bind(PROXY_ADDRESS)
        .map_err(|error| format!("cannot listen on {PROXY_ADDRESS} in the sandbox: {error}"))?;
    let mut forwards = Vec::with_capacity(forward_ports.len());
    for ForwardPort { service, port } in forward_ports {
        let address = forward_address(*port);
        let bound = TcpListener::bind(address).map_err(|error| {
            format!("service {service} cannot listen on {address} in the sandbox: {error}")
        })?;
        forwards.push(bound);
    }
    let folder = socket.parent().unwrap_or(Path::new("/"));
    let mut stream = in_folder(folder, SOCKET_NAME, UnixStream::connect)
        .map_err(|error| format!("cannot reach the launcher on {}: {error}", socket.display()))?;
    let failed = |error: std::io::Error| format!("cannot hand the proxy listener over: {error}");
    stream
        .set_read_timeout(Some(HANDOFF_TIMEOUT))
        .map_err(failed)?;
    let descriptors: Vec<_> = std::iter::once(&listener)
        .chain(&forwards)
        .map(AsFd::as_fd)
        .collect();
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1 + MAX_FORWARDS))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    if !control.push(SendAncillaryMessage::ScmRights(&descriptors)) {
        return Err("cannot hand the proxy listener over: no room for it".to_owned());
    }
    sendmsg(
        &stream,
        &[IoSlice::new(&[LISTENER_BYTE])],
        &mut control,
        SendFlags::empty(),
    )
    .map_err(|error| failed(error.into()))?;
    let mut answer = [0_u8; 1];
    stream.read_exact(&mut answer).map_err(failed)?;
    if answer != [RECEIVED_BYTE] {
        return Err("the launcher did not take the proxy listener".to_owned());
    }
    stream.set_read_timeout(None).map_err(failed)?;
    Ok(stream)
}

/// The binder: answers each of the launcher's requests on `stream` with a listener on the
/// sandbox's loopback at the port asked, until the launcher closes it. Keeps no copy.
pub fn serve_binds(stream: &UnixStream) -> Result<(), String> {
    let mut request = [0_u8; 3];
    loop {
        match (&*stream).read_exact(&mut request) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(format!("cannot read the launcher's request: {error}")),
        }
        let [BIND_BYTE, high, low] = request else {
            return Err(format!("the launcher asked {request:?}"));
        };
        let address = forward_address(u16::from_be_bytes([high, low]));
        let sent = match TcpListener::bind(address) {
            Ok(listener) => send_listener(stream, &listener),
            Err(error) => {
                let mut reason = format!("cannot listen on {address} in the sandbox: {error}");
                reason.truncate(MAX_REASON);
                let length = u16::try_from(reason.len()).unwrap_or(0).to_be_bytes();
                (&*stream)
                    .write_all(&[REFUSED_BYTE, length[0], length[1]])
                    .and_then(|()| (&*stream).write_all(reason.as_bytes()))
            }
        };
        sent.map_err(|error| format!("cannot answer the launcher: {error}"))?;
    }
}

fn send_listener(stream: &UnixStream, listener: &TcpListener) -> std::io::Result<()> {
    let descriptors = [listener.as_fd()];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    if !control.push(SendAncillaryMessage::ScmRights(&descriptors)) {
        return Err(std::io::Error::other("no room for the listener"));
    }
    sendmsg(
        stream,
        &[IoSlice::new(&[LISTENER_BYTE])],
        &mut control,
        SendFlags::empty(),
    )?;
    Ok(())
}

/// `act` on the socket `name` in `folder`, reached through the folder's open descriptor: a
/// socket's path is limited to 107 bytes, and the state folders of a long home are longer.
pub fn in_folder<T>(
    folder: &Path,
    name: impl AsRef<std::ffi::OsStr>,
    act: impl FnOnce(PathBuf) -> std::io::Result<T>,
) -> std::io::Result<T> {
    let opened = File::open(folder)?;
    act(PathBuf::from(format!("/proc/self/fd/{}", opened.as_raw_fd())).join(name.as_ref()))
}
