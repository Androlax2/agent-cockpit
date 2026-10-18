//! The command FIFOs of the resident desktop programs (osd, mpris-card), and who is at the other
//! end of the OSD's report socket.

use std::os::fd::OwnedFd;
use std::path::Path;

unsafe extern "C" {
    fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> i32;
}

/// The FIFO at `path`, created if missing (a plain file there is replaced), opened read-write and
/// non-blocking: with its own writer end open it never reads EOF when a client closes.
pub fn open(path: &Path) -> std::io::Result<OwnedFd> {
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
    if std::fs::metadata(path).is_ok_and(|metadata| !metadata.file_type().is_fifo()) {
        std::fs::remove_file(path)?;
    }
    if !path.exists() {
        let name = std::ffi::CString::new(path.to_string_lossy().as_bytes()).map_err(std::io::Error::other)?;
        // SAFETY: `name` is a valid NUL-terminated path for the duration of the call.
        if unsafe { mkfifo(name.as_ptr(), 0o600) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    const O_NONBLOCK: i32 = 0o4000;
    let file = std::fs::OpenOptions::new().read(true).write(true).custom_flags(O_NONBLOCK).open(path)?;
    Ok(OwnedFd::from(file))
}

#[repr(C)]
struct PeerCredentials {
    pid: i32,
    uid: u32,
    gid: u32,
}

unsafe extern "C" {
    fn getsockopt(socket: i32, level: i32, name: i32, value: *mut std::ffi::c_void, length: *mut u32) -> i32;
}

/// The pid of the process at the other end of a Unix socket (SO_PEERCRED), in the daemon's own pid
/// namespace: the kernel translates it, a sandbox cannot choose it.
pub fn peer_pid(stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    const SOL_SOCKET: i32 = 1;
    const SO_PEERCRED: i32 = 17;
    let mut credentials = PeerCredentials { pid: 0, uid: 0, gid: 0 };
    let mut length = std::mem::size_of::<PeerCredentials>() as u32;
    // SAFETY: credentials and length are valid for writes of the sizes passed.
    let result = unsafe { getsockopt(std::os::fd::AsRawFd::as_raw_fd(stream), SOL_SOCKET, SO_PEERCRED, (&mut credentials as *mut PeerCredentials).cast(), &mut length) };
    (result == 0 && credentials.pid > 0).then_some(credentials.pid as u32)
}


#[cfg(test)]
mod tests {
    #[test]
    fn the_peer_is_the_process_that_connected() {
        let path = std::env::temp_dir().join(format!("peer-{}.sock", cockpit::util::token_hex(4)));
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let _client = std::os::unix::net::UnixStream::connect(&path).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        assert_eq!(super::peer_pid(&accepted), Some(std::process::id()));
        std::fs::remove_file(path).unwrap();
    }
}
