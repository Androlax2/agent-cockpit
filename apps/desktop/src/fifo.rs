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

/// A connection's whole message: everything up to the sender's end of writing, within `deadline`
/// and `max_bytes`, else why not.
pub fn read_message(mut stream: std::os::unix::net::UnixStream, deadline: std::time::Duration, max_bytes: usize) -> Result<String, String> {
    use std::io::Read;
    let until = std::time::Instant::now() + deadline;
    let _ = stream.set_nonblocking(false);
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let left = until.checked_duration_since(std::time::Instant::now()).ok_or(format!("not finished within {deadline:?}"))?;
        let _ = stream.set_read_timeout(Some(left.max(std::time::Duration::from_millis(1))));
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Ok(count) if bytes.len() + count <= max_bytes => bytes.extend_from_slice(&chunk[..count]),
            Ok(_) => return Err(format!("more than {max_bytes} bytes")),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return Err(format!("not finished within {deadline:?}")),
        }
    }
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

    /// What read_message makes of a sender writing like `writing`, and how long the read took.
    fn message_after(writing: impl FnOnce(&mut std::os::unix::net::UnixStream) + Send + 'static) -> (Result<String, String>, std::time::Duration) {
        let (reading, mut sending) = std::os::unix::net::UnixStream::pair().unwrap();
        let writer = std::thread::spawn(move || writing(&mut sending));
        let started = std::time::Instant::now();
        let message = super::read_message(reading, std::time::Duration::from_millis(300), 16);
        let took = started.elapsed();
        let _ = writer.join();
        (message, took)
    }

    #[test]
    fn a_message_is_everything_until_the_sender_stops_writing() {
        use std::io::Write;
        let (message, _) = message_after(|stream| { stream.write_all(b"session {}\n").unwrap(); stream.shutdown(std::net::Shutdown::Write).unwrap(); });
        assert_eq!(message, Ok("session {}\n".to_string()));
    }

    #[test]
    fn a_sender_that_never_finishes_is_dropped_at_the_deadline() {
        // Security audit M5: one slow sandbox must not hold the daemon.
        use std::io::Write;
        let (message, took) = message_after(|stream| { stream.write_all(b"sess").unwrap(); std::thread::sleep(std::time::Duration::from_millis(600)); });
        assert!(message.is_err());
        assert!(took < std::time::Duration::from_millis(550), "waited {took:?}");
    }

    #[test]
    fn a_message_past_the_size_cap_is_dropped() {
        use std::io::Write;
        let (message, _) = message_after(|stream| { let _ = stream.write_all(&[b'x'; 64]); let _ = stream.shutdown(std::net::Shutdown::Write); });
        assert!(message.is_err());
    }
}
