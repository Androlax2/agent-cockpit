//! Reading a file an agent may have written, at the top of a checkout, without following it
//! anywhere else: readiness reads the project's compose files this way (#1371).

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use rustix::fs::{self, FileType, Mode, OFlags};
use rustix::io::Errno;

/// The text of the file `name` directly in `dir`, at most `max_bytes` long; None when there is
/// none. It is opened never through a symbolic link (`O_NOFOLLOW`) and without waiting on a
/// FIFO (`O_NONBLOCK`), and must be a plain file.
pub fn read_checkout_file(
    dir: &Path,
    name: &str,
    max_bytes: u64,
) -> Result<Option<String>, String> {
    if name.is_empty() || name.contains('/') || name == "." || name == ".." {
        return Err(format!("{name:?} is not a file name"));
    }
    let directory = fs::open(
        dir,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|errno| io::Error::from(errno).to_string())?;
    let opened = match fs::openat(
        &directory,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(opened) => opened,
        Err(Errno::NOENT) => return Ok(None),
        Err(Errno::LOOP) => return Err("it is a symbolic link, which is not followed".to_owned()),
        Err(errno) => return Err(io::Error::from(errno).to_string()),
    };
    let stat = fs::fstat(&opened).map_err(|errno| io::Error::from(errno).to_string())?;
    if !FileType::from_raw_mode(stat.st_mode).is_file() {
        return Err("it is not a plain file".to_owned());
    }
    if u64::try_from(stat.st_size).unwrap_or(u64::MAX) > max_bytes {
        return Err(format!("it is larger than {max_bytes} bytes"));
    }
    let mut text = String::new();
    File::from(opened)
        .take(max_bytes)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    Ok(Some(text))
}
