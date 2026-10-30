//! Peer-authenticated team IPC through an existing read-only answers mount.

use super::{AgentAction, ErrorCode, TeamError, TeamReply};
use crate::{approval, paths};
use std::fs;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const REQUEST_MAX: usize = 32768;
pub const AGENT_REPLY_MAX: usize = 65536;

pub fn socket_path() -> Result<PathBuf, TeamError> {
    let key = approval::own_answers_key().ok_or_else(|| {
        TeamError::new(
            ErrorCode::Unavailable,
            "team socket needs this project's read-only answers mount; restart the session",
        )
    })?;
    Ok(approval::answers_dir(&key, "teams").join("agent.sock"))
}

pub fn request(action: &AgentAction) -> Result<TeamReply, TeamError> {
    let wire = serde_json::to_vec(action)?;
    if wire.len() > REQUEST_MAX {
        return Err(TeamError::new(
            ErrorCode::Limit,
            "team request exceeds 32 KiB",
        ));
    }
    let path = socket_path()?;
    let mut stream = connect_through_restart(&path, CONNECT_PATIENCE)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(&wire)?;
    stream.shutdown(Shutdown::Write)?;
    let mut reply = Vec::new();
    (&mut stream)
        .take(AGENT_REPLY_MAX as u64 + 1)
        .read_to_end(&mut reply)
        .map_err(|error| {
            TeamError::new(
                ErrorCode::Unavailable,
                format!(
                    "team reply unavailable ({error}); resubmit only with the same submission key"
                ),
            )
        })?;
    if reply.len() > AGENT_REPLY_MAX {
        return Err(TeamError::new(
            ErrorCode::Limit,
            "team reply exceeds its limit",
        ));
    }
    serde_json::from_slice(&reply).map_err(|error| {
        TeamError::new(
            ErrorCode::Unavailable,
            format!("invalid team reply ({error}); resubmit only with the same submission key"),
        )
    })
}

/// Display refreshes have one deadline; action requests retain their existing transport.
pub(crate) fn host_request_until(
    path: &Path,
    request: &super::HostRequest,
    deadline: Instant,
) -> Result<TeamReply, TeamError> {
    let wire = serde_json::to_vec(request)?;
    let reply = exchange_until(path, &wire, deadline).map_err(|error| {
        TeamError::new(
            ErrorCode::Unavailable,
            format!("team display refresh failed: {error}"),
        )
    })?;
    decode_host_reply(&reply)
}

fn exchange_until(path: &Path, mut wire: &[u8], deadline: Instant) -> io::Result<Vec<u8>> {
    remaining_io_timeout(deadline)?;
    let mut stream = connect_display(path)?;
    while !wire.is_empty() {
        stream.set_write_timeout(Some(remaining_io_timeout(deadline)?))?;
        match stream.write(wire) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(written) => wire = &wire[written..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    stream.shutdown(Shutdown::Write)?;
    let mut reply = Vec::new();
    let mut limited = (&mut stream).take(HOST_REPLY_MAX as u64 + 1);
    let mut chunk = [0; 8192];
    loop {
        limited
            .get_ref()
            .set_read_timeout(Some(remaining_io_timeout(deadline)?))?;
        match limited.read(&mut chunk) {
            Ok(0) => return Ok(reply),
            Ok(read) => reply.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

fn remaining_io_timeout(deadline: Instant) -> io::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "team refresh deadline elapsed",
        ));
    }
    Ok(remaining.min(Duration::from_secs(3)))
}

fn connect_display(path: &Path) -> io::Result<UnixStream> {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: sockaddr_un consists of integer fields; zero also terminates sun_path.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid team socket path",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (destination, source) in address.sun_path.iter_mut().zip(bytes) {
        *destination = *source as libc::c_char;
    }
    // SAFETY: socket takes only integer arguments; a successful fd is owned below.
    let fd = unsafe {
        libc::socket(
            libc::AF_UNIX,
            libc::SOCK_STREAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: socket returned a new, uniquely owned stream fd, including on connect failure.
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    // SAFETY: address is initialized and lives through connect; length covers its bounded path.
    let connected = unsafe {
        libc::connect(
            fd,
            (&address as *const libc::sockaddr_un).cast(),
            length as libc::socklen_t,
        )
    };
    if connected != 0 {
        // A full Linux Unix-socket queue returns EAGAIN instead of pinning the display worker.
        return Err(io::Error::last_os_error());
    }
    stream.set_nonblocking(false)?;
    Ok(stream)
}

/// How long a refused connect is retried: an OSD restart binds and re-links the socket in seconds.
const CONNECT_PATIENCE: Duration = Duration::from_secs(15);

/// During a daemon restart the session's link still names the old socket, which refuses: nothing
/// was received, so only the connect is retried, never a request that was written. Each attempt
/// reopens the folder: the restarted daemon links its new socket there.
fn connect_through_restart(path: &Path, patience: Duration) -> Result<UnixStream, TeamError> {
    let deadline = std::time::Instant::now() + patience;
    loop {
        match connect_once(path) {
            Err(error) if is_restart_gap(&error) && std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(500))
            }
            Err(error) if is_restart_gap(&error) => {
                return Err(TeamError::new(
                    ErrorCode::Unavailable,
                    format!(
                        "team socket cannot connect for {} s ({error}); nothing was sent: retry with the same submission key once the OSD daemon runs",
                        patience.as_secs()
                    ),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(TeamError::new(
                    ErrorCode::Unavailable,
                    format!(
                        "team socket unavailable ({error}); reload the cockpit and restart this session if needed"
                    ),
                ));
            }
            Err(error) => {
                return Err(TeamError::new(
                    ErrorCode::Unavailable,
                    format!("team socket cannot connect: {error}"),
                ));
            }
            Ok(Err(refusal)) => return Err(refusal),
            Ok(Ok(stream)) => return Ok(stream),
        }
    }
}

fn is_restart_gap(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::ConnectionRefused
}

/// The outer error is a connect failure worth retrying; the inner one a refusal that is final.
fn connect_once(path: &Path) -> std::io::Result<Result<UnixStream, TeamError>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() {
        return Ok(Err(TeamError::new(
            ErrorCode::Unavailable,
            "team socket must be a real socket, not a symlink or regular file",
        )));
    }
    // Project answer slugs can exceed AF_UNIX's 108-byte pathname limit. Use only this opened directory.
    let Some(parent) = path.parent() else {
        return Ok(Err(TeamError::new(
            ErrorCode::Storage,
            "invalid team socket path",
        )));
    };
    let folder = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(parent)?;
    let endpoint = format!("/proc/self/fd/{}/agent.sock", folder.as_raw_fd());
    UnixStream::connect(endpoint).map(Ok)
}

pub const HOST_REPLY_MAX: usize = 1 << 20;

/// The cockpit's host request; a missing or slow daemon is an error, never an optimistic grant.
pub fn host_request(request: &super::HostRequest) -> Result<TeamReply, TeamError> {
    let path = paths::team_host_socket();
    host_request_at(&path, request)
}

pub(crate) fn host_request_at(
    path: &Path,
    request: &super::HostRequest,
) -> Result<TeamReply, TeamError> {
    let mut stream = UnixStream::connect(path).map_err(|error| {
        TeamError::new(
            ErrorCode::Unavailable,
            format!("team service unavailable ({error}); is the OSD daemon running?"),
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;
    stream.write_all(&serde_json::to_vec(request)?)?;
    stream.shutdown(Shutdown::Write)?;
    let mut reply = Vec::new();
    (&mut stream)
        .take(HOST_REPLY_MAX as u64 + 1)
        .read_to_end(&mut reply)
        .map_err(|error| {
            TeamError::new(
                ErrorCode::Unavailable,
                format!("team service did not answer: {error}"),
            )
        })?;
    decode_host_reply(&reply)
}

fn decode_host_reply(reply: &[u8]) -> Result<TeamReply, TeamError> {
    serde_json::from_slice(reply).map_err(|error| {
        TeamError::new(
            ErrorCode::Unavailable,
            format!("invalid team service reply: {error}"),
        )
    })
}

/// A hard link preserves the host listener inode; no writable global socket path is followed.
pub fn publish_socket(source: &Path, answers_root: &Path) -> Result<(), TeamError> {
    if !fs::symlink_metadata(source)?.file_type().is_socket() {
        return Err(TeamError::new(
            ErrorCode::Storage,
            "private team listener is not a socket",
        ));
    }
    let root = fs::symlink_metadata(answers_root)?;
    if !root.is_dir() || root.file_type().is_symlink() {
        return Err(TeamError::new(
            ErrorCode::Storage,
            "answers root must be an existing real directory",
        ));
    }
    let folder = answers_root.join("teams");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&folder)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                Ok(())
            } else {
                Err(error)
            }
        })?;
    let metadata = fs::symlink_metadata(&folder)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(TeamError::new(
            ErrorCode::Storage,
            "team answers folder is not a real directory",
        ));
    }
    let target = folder.join("agent.sock");
    match fs::symlink_metadata(&target) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() {
                return Err(TeamError::new(
                    ErrorCode::Storage,
                    "refusing to replace an unfamiliar team socket entry",
                ));
            }
            let original = fs::metadata(source)?;
            if metadata.ino() == original.ino() && metadata.dev() == original.dev() {
                return Ok(());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let temporary = folder.join(format!(".agent-{}.sock", crate::util::token_hex(8)));
    fs::hard_link(source, &temporary)?;
    if let Err(error) = fs::rename(&temporary, target) {
        let cleanup = fs::remove_file(&temporary);
        return Err(TeamError::new(
            ErrorCode::Storage,
            format!("cannot publish team socket: {error}; cleanup: {cleanup:?}"),
        ));
    }
    Ok(())
}

pub fn prepare() -> Result<(), TeamError> {
    let key = approval::own_answers_key()
        .ok_or_else(|| TeamError::new(ErrorCode::Unavailable, "no project answers key"))?;
    let root = approval::answers_dir(&key, "teams")
        .parent()
        .ok_or_else(|| TeamError::new(ErrorCode::Storage, "invalid answers path"))?
        .to_path_buf();
    publish_socket(&paths::team_agent_socket(), &root)
}
