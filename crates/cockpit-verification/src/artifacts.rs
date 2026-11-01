//! Artifact fingerprints: the SHA-256 of a file inside a checkout. Reading files is the only
//! I/O here besides the database; no program runs.

use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::path::Path;

use rustix::fs::{self, FileType, Mode, OFlags};
use rustix::io::Errno;
use sha2::{Digest, Sha256};

use crate::error::invalid;
use crate::{ARTIFACTS_MAX, Artifact, PATH_MAX_CHARS, Refusal};

/// The fingerprints of `paths`, relative to `checkout`, ordered by path. Take them before
/// the run starts. Refused unless each names an existing file inside the checkout, once.
pub fn fingerprint(checkout: &Path, paths: &[&str]) -> Result<Vec<Artifact>, Refusal> {
    let mut sorted = paths.to_vec();
    sorted.sort_unstable();
    check_artifact_paths(&sorted)?;
    let checkout = Checkout::open(checkout)?;
    sorted
        .into_iter()
        .map(|path| {
            let digest = checkout
                .digest(path)
                .map_err(|reason| invalid("artifacts", format!("{path}: {reason}")))?
                .ok_or_else(|| invalid("artifacts", format!("{path}: does not exist")))?;
            Ok(Artifact {
                path: path.to_owned(),
                digest,
            })
        })
        .collect()
}

/// Refuses more than [`ARTIFACTS_MAX`] paths, a path that is not plainly relative, and one
/// named twice. `sorted` is ordered, so a repeat is adjacent.
pub(crate) fn check_artifact_paths(sorted: &[&str]) -> Result<(), Refusal> {
    if sorted.len() > ARTIFACTS_MAX {
        return Err(invalid("artifacts", format!("at most {ARTIFACTS_MAX}")));
    }
    for path in sorted {
        check_relative(path).map_err(|reason| invalid("artifacts", format!("{path}: {reason}")))?;
    }
    if let Some(pair) = sorted.windows(2).find(|pair| pair[0] == pair[1]) {
        return Err(invalid("artifacts", format!("{} is named twice", pair[0])));
    }
    Ok(())
}

/// A checkout root, opened once: every artifact is opened below this descriptor.
pub(crate) struct Checkout {
    root: OwnedFd,
}

impl Checkout {
    /// Refused unless `path` opens as a directory, so a wrong checkout is named as such
    /// instead of showing every artifact as missing.
    pub(crate) fn open(path: &Path) -> Result<Checkout, Refusal> {
        let root = fs::open(
            path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|errno| match errno {
            Errno::NOTDIR => invalid("checkout", "is not a directory"),
            _ => invalid(
                "checkout",
                format!("cannot be opened: {}", io::Error::from(errno)),
            ),
        })?;
        Ok(Checkout { root })
    }

    /// The digest of `path` now: `None` when nothing is there, the reason when it is not a
    /// readable file inside the checkout.
    pub(crate) fn digest(&self, path: &str) -> Result<Option<String>, String> {
        self.digest_between_segments(path, &mut || {})
    }

    /// [`Checkout::digest`], running `between_segments` before each segment is opened: the
    /// tests change the tree there.
    fn digest_between_segments(
        &self,
        path: &str,
        between_segments: &mut dyn FnMut(),
    ) -> Result<Option<String>, String> {
        check_relative(path)?;
        let Some(file) = self.open_file(path, between_segments)? else {
            return Ok(None);
        };
        sha256_of(file).map(Some).map_err(|error| error.to_string())
    }

    /// Opens `path` one segment at a time, each relative to the directory opened before it
    /// and never through a symbolic link (`O_NOFOLLOW`). Nothing is resolved by name after
    /// being checked, so swapping a segment for a link cannot lead the open out of the
    /// checkout.
    fn open_file(
        &self,
        path: &str,
        between_segments: &mut dyn FnMut(),
    ) -> Result<Option<File>, String> {
        let segments: Vec<&str> = path.split('/').collect();
        let (name, directories) = segments
            .split_last()
            .expect("check_relative refuses an empty path");
        let mut parent: Option<OwnedFd> = None;
        for directory in directories {
            between_segments();
            let Some((opened, kind)) =
                open_segment(parent.as_ref().unwrap_or(&self.root), directory)?
            else {
                return Ok(None);
            };
            // A parent replaced by a file is as gone as a removed file.
            if !kind.is_dir() {
                return Ok(None);
            }
            parent = Some(opened);
        }
        between_segments();
        let Some((opened, kind)) = open_segment(parent.as_ref().unwrap_or(&self.root), name)?
        else {
            return Ok(None);
        };
        if !kind.is_file() {
            return Err("is not a file".to_owned());
        }
        Ok(Some(File::from(opened)))
    }
}

/// `name` in `parent` and what it is, `None` when nothing is there. Not `O_DIRECTORY`: Linux
/// reports a link refused under it as "not a directory", which would hide the link.
/// Non-blocking, so a FIFO put in a file's place is reported instead of waited on.
fn open_segment(parent: &OwnedFd, name: &str) -> Result<Option<(OwnedFd, FileType)>, String> {
    let opened = fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    );
    let opened = match opened {
        Ok(opened) => opened,
        Err(Errno::NOENT | Errno::NOTDIR) => return Ok(None),
        Err(Errno::LOOP) => {
            return Err("goes through a symbolic link, which is not followed".to_owned());
        }
        Err(errno) => return Err(io::Error::from(errno).to_string()),
    };
    let stat = fs::fstat(&opened).map_err(|errno| io::Error::from(errno).to_string())?;
    Ok(Some((opened, FileType::from_raw_mode(stat.st_mode))))
}

fn sha256_of(mut file: File) -> io::Result<String> {
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Refuses an empty, absolute, too long or non-normalized path: `/`-separated segments,
/// none empty, `.` or `..`, so one file has one spelling.
pub(crate) fn check_relative(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("is empty".to_owned());
    }
    if path.chars().count() > PATH_MAX_CHARS {
        return Err(format!("is longer than {PATH_MAX_CHARS} characters"));
    }
    let is_normalized = path
        .split('/')
        .all(|segment| !segment.is_empty() && segment != "." && segment != "..");
    if !is_normalized {
        return Err(
            "must be relative to the checkout, `/`-separated, without empty, `.` or `..` segments"
                .to_owned(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    use super::*;

    /// A checkout holding `src/lib.rs` and, beside it, `outside/lib.rs`.
    struct Tree {
        dir: tempfile::TempDir,
    }

    impl Tree {
        fn new() -> Tree {
            let dir = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(dir.path().join("checkout/src")).unwrap();
            std::fs::write(dir.path().join("checkout/src/lib.rs"), "inside\n").unwrap();
            std::fs::create_dir(dir.path().join("outside")).unwrap();
            std::fs::write(dir.path().join("outside/lib.rs"), "outside\n").unwrap();
            Tree { dir }
        }

        fn path(&self, relative: &str) -> PathBuf {
            self.dir.path().join(relative)
        }

        fn digest_of(&self, relative: &str) -> String {
            sha256_of(File::open(self.path(relative)).unwrap()).unwrap()
        }

        /// Moves `checkout/<relative>` aside and puts a link to `outside/<target>` there.
        fn swap_for_link(&self, relative: &str, target: &str) {
            let original = self.path(&format!("checkout/{relative}"));
            std::fs::rename(&original, self.path("checkout/moved-aside")).unwrap();
            symlink(self.path(&format!("outside/{target}")), original).unwrap();
        }

        /// The digest of `src/lib.rs`, with `swap` run before segment `swap_before` opens.
        fn digest_swapping(
            &self,
            swap_before: usize,
            swap: impl Fn(),
        ) -> Result<Option<String>, String> {
            let checkout = Checkout::open(&self.path("checkout")).unwrap();
            let mut segment = 0;
            checkout.digest_between_segments("src/lib.rs", &mut || {
                if segment == swap_before {
                    swap();
                }
                segment += 1;
            })
        }
    }

    #[test]
    fn a_directory_swapped_for_a_link_while_opening_never_yields_an_outside_digest() {
        for swap_before in 0..2 {
            let tree = Tree::new();
            let outside = tree.digest_of("outside/lib.rs");
            let digest = tree.digest_swapping(swap_before, || tree.swap_for_link("src", "."));
            assert_ne!(
                digest,
                Ok(Some(outside)),
                "swapped before segment {swap_before}"
            );
            if swap_before == 0 {
                assert_eq!(
                    digest,
                    Err("goes through a symbolic link, which is not followed".to_owned())
                );
            } else {
                // `src` was already open: the file read is the one moved aside in the checkout.
                assert_eq!(
                    digest,
                    Ok(Some(tree.digest_of("checkout/moved-aside/lib.rs")))
                );
            }
        }
    }

    #[test]
    fn a_file_swapped_for_a_link_while_opening_never_yields_an_outside_digest() {
        let tree = Tree::new();
        let digest = tree.digest_swapping(1, || tree.swap_for_link("src/lib.rs", "lib.rs"));
        assert_eq!(
            digest,
            Err("goes through a symbolic link, which is not followed".to_owned())
        );
    }
}
