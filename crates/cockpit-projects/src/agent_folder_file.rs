//! Writing a file in a folder an agent may have written, without following the folder or the
//! file anywhere else: the daemon writes a sandboxed session's Docker client configuration
//! this way (#390 part 6d), as the owner, in a folder the session's agent writes too.

use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use rustix::fs::{self, AtFlags, Mode, OFlags};
use rustix::io::Errno;

/// Writes `contents` as the file `name` in the folder `folder` directly in `parent`, or
/// removes it when `contents` is None. `parent` is trusted; `folder` is opened never through
/// a symbolic link (`O_NOFOLLOW`). The file is written under a new name and renamed over
/// `name`, so whatever `name` was, a symbolic or hard link included, is replaced and never
/// written through.
pub fn replace_agent_folder_file(
    parent: &Path,
    folder: &str,
    name: &str,
    contents: Option<&str>,
) -> io::Result<()> {
    for component in [folder, name] {
        if component.is_empty() || component.contains('/') || component == "." || component == ".."
        {
            return Err(io::Error::other(format!(
                "{component:?} is not a file name"
            )));
        }
    }
    let parent = fs::open(
        parent,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let folder = match fs::openat(
        &parent,
        folder,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(folder) => folder,
        Err(Errno::LOOP | Errno::NOTDIR) => {
            return Err(io::Error::other(format!(
                "{folder} is not a folder of its own: a symbolic link is not followed"
            )));
        }
        Err(errno) => return Err(errno.into()),
    };
    let Some(contents) = contents else {
        return match fs::unlinkat(&folder, name, AtFlags::empty()) {
            Ok(()) | Err(Errno::NOENT) => Ok(()),
            Err(errno) => Err(errno.into()),
        };
    };
    let staged = format!(".{name}.torlek-new");
    match fs::unlinkat(&folder, staged.as_str(), AtFlags::empty()) {
        Ok(()) | Err(Errno::NOENT) => {}
        Err(errno) => return Err(errno.into()),
    }
    let opened = fs::openat(
        &folder,
        staged.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )?;
    let mut file = File::from(opened);
    file.write_all(contents.as_bytes())?;
    file.sync_all()?;
    fs::renameat(&folder, staged.as_str(), &folder, name)?;
    Ok(())
}
