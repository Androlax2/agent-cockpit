//! The command FIFOs of the resident desktop programs (osd, mpris-card).

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
