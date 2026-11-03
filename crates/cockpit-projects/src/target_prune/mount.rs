//! A mount proof is mandatory; device numbers alone cannot distinguish bind mounts.

use std::os::fd::{AsFd, OwnedFd};

/// Keep Linux-only flags behind a compile-time boundary. Unsupported platforms
/// cannot turn an unavailable no-automount observation into an absent entry.
#[cfg(target_os = "linux")]
pub(super) fn stat_child(
    parent: impl AsFd,
    name: &std::ffi::CStr,
) -> rustix::io::Result<rustix::fs::Stat> {
    use rustix::fs::AtFlags;
    rustix::fs::statat(
        parent,
        name,
        AtFlags::SYMLINK_NOFOLLOW | AtFlags::NO_AUTOMOUNT,
    )
}

#[cfg(not(target_os = "linux"))]
pub(super) fn stat_child(
    _parent: impl AsFd,
    _name: &std::ffi::CStr,
) -> rustix::io::Result<rustix::fs::Stat> {
    Err(rustix::io::Errno::NOSYS)
}

#[cfg(target_os = "linux")]
pub(super) fn mount_id(fd: impl AsFd) -> Result<u64, String> {
    use rustix::fs::{AtFlags, StatxFlags};
    let stat = rustix::fs::statx(
        fd,
        c"",
        AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW | AtFlags::NO_AUTOMOUNT,
        StatxFlags::MNT_ID,
    )
    .map_err(|error| format!("mount identity unavailable: {error}"))?;
    proven_mount_id(stat.stx_mask, stat.stx_mnt_id)
}

#[cfg(target_os = "linux")]
fn proven_mount_id(mask: u32, id: u64) -> Result<u64, String> {
    if mask & rustix::fs::StatxFlags::MNT_ID.bits() == 0 {
        return Err("filesystem did not provide mount identity".to_owned());
    }
    Ok(id)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn mount_id(_fd: impl AsFd) -> Result<u64, String> {
    Err("target pruning requires verified Linux mount-boundary support".to_owned())
}

#[cfg(target_os = "linux")]
pub(super) fn open_child(
    parent: impl AsFd,
    name: &std::ffi::CStr,
    directory: bool,
    expected_mount: u64,
) -> Result<OwnedFd, String> {
    use rustix::fs::{FileType, Mode, OFlags, ResolveFlags};
    let bytes = name.to_bytes();
    if bytes.is_empty() || bytes == b"." || bytes == b".." || bytes.contains(&b'/') {
        return Err("a cleanup entry must be one relative name".to_owned());
    }
    let flags = OFlags::CLOEXEC
        | OFlags::NOFOLLOW
        | if directory {
            OFlags::RDONLY | OFlags::DIRECTORY
        } else {
            // Pin metadata without reading contents or opening a device/FIFO for I/O.
            OFlags::PATH
        };
    let opened = rustix::fs::openat2(
        parent,
        name,
        flags,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_XDEV
            | ResolveFlags::NO_SYMLINKS
            | ResolveFlags::NO_MAGICLINKS,
    )
    .map_err(|error| format!("cannot prove a no-mount, no-link child: {error}"))?;
    let stat = rustix::fs::fstat(&opened).map_err(|error| error.to_string())?;
    let kind = FileType::from_raw_mode(stat.st_mode);
    // O_PATH|O_NOFOLLOW may return a descriptor to the symlink itself; reject it.
    if !(kind.is_dir() || kind.is_file()) || mount_id(&opened)? != expected_mount {
        return Err("child is not regular build output on the approved mount".to_owned());
    }
    Ok(opened)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn open_child(
    _parent: impl AsFd,
    _name: &std::ffi::CStr,
    _directory: bool,
    _expected_mount: u64,
) -> Result<OwnedFd, String> {
    Err("target pruning requires verified Linux mount-boundary support".to_owned())
}

#[cfg(target_os = "linux")]
pub(super) fn identity(fd: impl AsFd) -> Result<(String, u64, bool), String> {
    use rustix::fs::{AtFlags, FileType, StatxFlags};
    let required = StatxFlags::TYPE
        | StatxFlags::NLINK
        | StatxFlags::INO
        | StatxFlags::SIZE
        | StatxFlags::MTIME
        | StatxFlags::CTIME
        | StatxFlags::MNT_ID;
    let stat = rustix::fs::statx(
        fd,
        c"",
        AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT,
        required,
    )
    .map_err(|error| format!("cleanup metadata unavailable: {error}"))?;
    if stat.stx_mask & required.bits() != required.bits() {
        return Err("filesystem did not supply complete cleanup identity".to_owned());
    }
    let directory = FileType::from_raw_mode(u32::from(stat.stx_mode)).is_dir();
    let file = FileType::from_raw_mode(u32::from(stat.stx_mode)).is_file();
    if !directory && (!file || stat.stx_nlink != 1) {
        return Err("cleanup refuses links, hardlinks and special files".to_owned());
    }
    let mut id = format!(
        "{}:{}:{}:{}:{}",
        if directory { "d" } else { "f" },
        stat.stx_dev_major,
        stat.stx_dev_minor,
        stat.stx_ino,
        stat.stx_mnt_id
    );
    if !directory {
        use std::fmt::Write;
        write!(
            &mut id,
            ":{}:{}:{}:{}:{}",
            stat.stx_size,
            stat.stx_mtime.tv_sec,
            stat.stx_mtime.tv_nsec,
            stat.stx_ctime.tv_sec,
            stat.stx_ctime.tv_nsec
        )
        .map_err(|e| e.to_string())?;
    }
    Ok((id, if directory { 0 } else { stat.stx_size }, directory))
}

#[cfg(not(target_os = "linux"))]
pub(super) fn identity(_fd: impl AsFd) -> Result<(String, u64, bool), String> {
    Err("target pruning requires verified Linux mount-boundary support".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn mount_boundary_opens_only_fixture_descendants() {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::create_dir(fixture.path().join("target")).unwrap();
        std::fs::write(fixture.path().join("target/artifact"), b"fixture").unwrap();
        let root = std::fs::File::open(fixture.path()).unwrap();
        let id = mount_id(&root).expect("fixture filesystem supplies mount identity");
        let target = open_child(&root, c"target", true, id)
            .expect("kernel can enforce no mount crossing for an ordinary fixture directory");
        let artifact = open_child(&target, c"artifact", false, id).unwrap();
        assert_eq!(rustix::fs::fstat(&artifact).unwrap().st_size, 7);
        assert_eq!(stat_child(&target, c"artifact").unwrap().st_size, 7);
        assert!(matches!(
            stat_child(&target, c"absent"),
            Err(rustix::io::Errno::NOENT)
        ));
        assert!(open_child(&target, c"../target/artifact", false, id).is_err());
        assert!(open_child(&target, c"/artifact", false, id).is_err());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn mount_boundary_refuses_symlinks_and_wrong_mount_identity() {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::create_dir(fixture.path().join("target")).unwrap();
        std::os::unix::fs::symlink("target", fixture.path().join("link")).unwrap();
        let root = std::fs::File::open(fixture.path()).unwrap();
        let id = mount_id(&root).unwrap();
        assert!(open_child(&root, c"link", true, id).is_err());
        assert!(open_child(&root, c"link", false, id).is_err());
        assert!(
            rustix::fs::FileType::from_raw_mode(stat_child(&root, c"link").unwrap().st_mode)
                .is_symlink()
        );
        assert!(open_child(&root, c"target", true, id.wrapping_add(1)).is_err());
        assert!(fixture.path().join("target").is_dir());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn mount_boundary_unknown_metadata_is_never_zero_or_same_mount() {
        assert!(proven_mount_id(0, 123).is_err());
        assert!(proven_mount_id(rustix::fs::StatxFlags::BASIC_STATS.bits(), 123).is_err());
        assert_eq!(
            proven_mount_id(rustix::fs::StatxFlags::MNT_ID.bits(), 123),
            Ok(123)
        );
    }

    #[test]
    #[cfg(not(target_os = "linux"))]
    fn mount_boundary_unsupported_platform_refuses_without_effects() {
        let fixture = tempfile::tempdir().unwrap();
        let root = std::fs::File::open(fixture.path()).unwrap();
        assert!(mount_id(&root).is_err());
        assert!(open_child(&root, c"target", true, 1).is_err());
        assert!(matches!(
            stat_child(&root, c"absent"),
            Err(rustix::io::Errno::NOSYS)
        ));
        assert_eq!(std::fs::read_dir(fixture.path()).unwrap().count(), 0);
    }
}
