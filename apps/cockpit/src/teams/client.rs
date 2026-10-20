//! Peer-authenticated team IPC through an existing read-only answers mount.

use super::{AgentAction, ErrorCode, TeamError, TeamReply};
use crate::{approval, paths};
use std::fs;
use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const REQUEST_MAX: usize = 32768;
pub const AGENT_REPLY_MAX: usize = 65536;

pub fn socket_path() -> Result<PathBuf, TeamError> {
    let key = approval::own_answers_key().ok_or_else(|| TeamError::new(ErrorCode::Unavailable, "team socket needs this project's read-only answers mount; restart the session"))?;
    Ok(approval::answers_dir(&key, "teams").join("agent.sock"))
}

pub fn request(action: &AgentAction) -> Result<TeamReply, TeamError> {
    let wire = serde_json::to_vec(action)?;
    if wire.len() > REQUEST_MAX { return Err(TeamError::new(ErrorCode::Limit, "team request exceeds 32 KiB")); }
    let path = socket_path()?;
    let metadata = fs::symlink_metadata(&path).map_err(|error| TeamError::new(ErrorCode::Unavailable, format!("team socket unavailable ({error}); reload the cockpit and restart this session if needed")))?;
    if !metadata.file_type().is_socket() { return Err(TeamError::new(ErrorCode::Unavailable, "team socket must be a real socket, not a symlink or regular file")); }
    // Project answer slugs can exceed AF_UNIX's 108-byte pathname limit. Use only this opened directory.
    let folder = fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path.parent().ok_or_else(|| TeamError::new(ErrorCode::Storage, "invalid team socket path"))?)?;
    let endpoint = format!("/proc/self/fd/{}/agent.sock", folder.as_raw_fd());
    let mut stream = UnixStream::connect(endpoint).map_err(|error| TeamError::new(ErrorCode::Unavailable, format!("team socket cannot connect: {error}")))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(&wire)?;
    stream.shutdown(Shutdown::Write)?;
    let mut reply = Vec::new();
    (&mut stream).take(AGENT_REPLY_MAX as u64 + 1).read_to_end(&mut reply)
        .map_err(|error| TeamError::new(ErrorCode::Unavailable, format!("team reply unavailable ({error}); resubmit only with the same submission key")))?;
    if reply.len() > AGENT_REPLY_MAX { return Err(TeamError::new(ErrorCode::Limit, "team reply exceeds its limit")); }
    serde_json::from_slice(&reply).map_err(|error| TeamError::new(ErrorCode::Unavailable, format!("invalid team reply ({error}); resubmit only with the same submission key")))
}

/// A hard link preserves the host listener inode; no writable global socket path is followed.
pub fn publish_socket(source: &Path, answers_root: &Path) -> Result<(), TeamError> {
    if !fs::symlink_metadata(source)?.file_type().is_socket() {
        return Err(TeamError::new(ErrorCode::Storage, "private team listener is not a socket"));
    }
    let root = fs::symlink_metadata(answers_root)?;
    if !root.is_dir() || root.file_type().is_symlink() {
        return Err(TeamError::new(ErrorCode::Storage, "answers root must be an existing real directory"));
    }
    let folder = answers_root.join("teams");
    fs::DirBuilder::new().mode(0o700).create(&folder).or_else(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists { Ok(()) } else { Err(error) }
    })?;
    let metadata = fs::symlink_metadata(&folder)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(TeamError::new(ErrorCode::Storage, "team answers folder is not a real directory"));
    }
    let target = folder.join("agent.sock");
    match fs::symlink_metadata(&target) {
        Ok(metadata) => {
            if !metadata.file_type().is_socket() { return Err(TeamError::new(ErrorCode::Storage, "refusing to replace an unfamiliar team socket entry")); }
            let original = fs::metadata(source)?;
            if metadata.ino() == original.ino() && metadata.dev() == original.dev() { return Ok(()); }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
        Err(error) => return Err(error.into()),
    }
    let temporary = folder.join(format!(".agent-{}.sock", crate::util::token_hex(8)));
    fs::hard_link(source, &temporary)?;
    if let Err(error) = fs::rename(&temporary, target) {
        let cleanup = fs::remove_file(&temporary);
        return Err(TeamError::new(ErrorCode::Storage, format!("cannot publish team socket: {error}; cleanup: {cleanup:?}")));
    }
    Ok(())
}

pub fn prepare() -> Result<(), TeamError> {
    let key = approval::own_answers_key().ok_or_else(|| TeamError::new(ErrorCode::Unavailable, "no project answers key"))?;
    let root = approval::answers_dir(&key, "teams").parent().ok_or_else(|| TeamError::new(ErrorCode::Storage, "invalid answers path"))?.to_path_buf();
    publish_socket(&paths::team_agent_socket(), &root)
}
