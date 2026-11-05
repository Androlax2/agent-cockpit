//! The proxy listener's way out of the sandbox (#546 M4 A): stage 2 creates it on the
//! loopback of the sandbox's empty network namespace, then passes it to the launcher, which
//! serves it from the host's network. The agent connects to an address inside its own
//! namespace; nothing of the host's network is in there.
//!
//! The hand-off goes through a named Unix socket in a folder of the launcher's own, bound
//! read-only into the view and never granted by Landlock: stage 2 connects before the
//! program runs, and the launcher removes the socket as soon as it has the listener.

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

use cockpit_sandbox::PROXY_ADDRESS;
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
        let listener = in_folder(&folder, UnixListener::bind)
            .map_err(|error| format!("cannot listen on {}: {error}", socket.display()))?;
        Ok(Waiting { folder, listener })
    }

    pub fn socket(&self) -> PathBuf {
        self.folder.join(SOCKET_NAME)
    }

    /// Stage 2's listener, once it sent it; None when `child` ended first, so the launcher
    /// reports the child's own failure.
    pub fn receive(self, child: &mut Child) -> Result<Option<TcpListener>, String> {
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
        take_listener(stream).map(Some)
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        // Nothing else lives there; a folder left behind is replaced at the next launch.
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

fn take_listener(mut stream: UnixStream) -> Result<TcpListener, String> {
    let failed = |error: std::io::Error| format!("cannot take the proxy listener: {error}");
    stream.set_nonblocking(false).map_err(failed)?;
    stream
        .set_read_timeout(Some(HANDOFF_TIMEOUT))
        .map_err(failed)?;
    let mut byte = [0_u8; 1];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    recvmsg(
        &stream,
        &mut [IoSliceMut::new(&mut byte)],
        &mut control,
        RecvFlags::CMSG_CLOEXEC,
    )
    .map_err(|error| failed(error.into()))?;
    let mut received = Vec::new();
    for message in control.drain() {
        if let RecvAncillaryMessage::ScmRights(descriptors) = message {
            received.extend(descriptors);
        }
    }
    let (Some(descriptor), true, [LISTENER_BYTE]) = (received.pop(), received.is_empty(), byte)
    else {
        return Err("stage 2 sent something other than one proxy listener".to_owned());
    };
    let listener = TcpListener::from(descriptor);
    let expected: SocketAddr = PROXY_ADDRESS
        .parse()
        .expect("the proxy address is a socket address");
    match listener.local_addr() {
        Ok(address) if address == expected => {}
        other => {
            return Err(format!(
                "stage 2 sent a listener on {other:?}, not on {PROXY_ADDRESS}"
            ));
        }
    }
    stream.write_all(&[RECEIVED_BYTE]).map_err(failed)?;
    Ok(listener)
}

/// Stage 2's side, before Landlock and before the program: listens on the proxy address of
/// the sandbox's loopback and sends the listener to the launcher on `socket`. Stage 2 keeps
/// no copy: the program never holds it.
pub fn hand_over(socket: &Path) -> Result<(), String> {
    let listener = TcpListener::bind(PROXY_ADDRESS)
        .map_err(|error| format!("cannot listen on {PROXY_ADDRESS} in the sandbox: {error}"))?;
    let folder = socket.parent().unwrap_or(Path::new("/"));
    let mut stream = in_folder(folder, UnixStream::connect)
        .map_err(|error| format!("cannot reach the launcher on {}: {error}", socket.display()))?;
    let failed = |error: std::io::Error| format!("cannot hand the proxy listener over: {error}");
    stream
        .set_read_timeout(Some(HANDOFF_TIMEOUT))
        .map_err(failed)?;
    let descriptors = [listener.as_fd()];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(1))];
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
    Ok(())
}

/// `act` on the hand-off socket's name in `folder`, reached through the folder's open
/// descriptor: a socket's path is limited to 107 bytes, and the state folders of a long
/// home are longer.
fn in_folder<T>(
    folder: &Path,
    act: impl FnOnce(PathBuf) -> std::io::Result<T>,
) -> std::io::Result<T> {
    let opened = File::open(folder)?;
    act(PathBuf::from(format!(
        "/proc/self/fd/{}/{SOCKET_NAME}",
        opened.as_raw_fd()
    )))
}
