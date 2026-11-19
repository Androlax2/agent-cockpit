//! Where the root-run helper is installed (#1387, Theo 2026-11-19): a package's
//! `torlek-sandbox` under `/usr/lib/torlek` is the helper as it is; on the install script's
//! path, which never calls sudo, `torlek setup root-run` copies the owner's `torlek-sandbox` to
//! [`SET_UP_ROOT_RUN_HELPER`], root's alone and apart from the masks helper's copy. The package's
//! wins where both are. sudo runs the helper as root, so only a file root alone can change, in
//! folders root alone can change, is the helper: anything else could have been replaced by
//! what the owner's account runs.

use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// The helper a package installs: the release bundle's own `torlek-sandbox`.
pub const PACKAGED_ROOT_RUN_HELPER: &str = "/usr/lib/torlek/bin/torlek-sandbox";

/// The setup step's root-owned copy of the owner's `torlek-sandbox`.
pub const SET_UP_ROOT_RUN_HELPER: &str = "/usr/local/lib/torlek/root-run/torlek-sandbox";

/// The installed helper sudo is to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootRunHelper {
    /// Its path on the host.
    pub path: PathBuf,
    /// A package's, or else the setup step's.
    pub is_packaged: bool,
}

/// Why no installed helper is the one to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootRunUnready {
    /// Neither a package nor `torlek setup root-run` installed one.
    NotInstalled,
    /// The setup step's copy is not the `torlek-sandbox` the daemon runs: an earlier release's.
    OutOfDate { path: PathBuf },
    /// A file is at a helper's path, but more than root could have written it.
    Untrusted { path: PathBuf, reason: String },
    /// What is installed cannot be read.
    Unknown(String),
}

/// The helper under `root` (`/` but in tests) sudo is to run: the package's when it is there,
/// else the setup step's when it has `current_helper`'s contents, the `torlek-sandbox` the
/// daemon runs. Each must be a regular file that neither it nor a folder above it lets
/// another user than `root_uid` (0 but in tests) change, and that is neither setuid nor
/// setgid. A package's needs no comparison: the package manager keeps it current.
pub fn root_run_helper(
    root: &Path,
    current_helper: &Path,
    root_uid: u32,
) -> Result<RootRunHelper, RootRunUnready> {
    let at = |path: &str| root.join(path.trim_start_matches('/'));
    for (path, is_packaged) in [
        (PACKAGED_ROOT_RUN_HELPER, true),
        (SET_UP_ROOT_RUN_HELPER, false),
    ] {
        let found = at(path);
        let metadata = match std::fs::symlink_metadata(&found) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(RootRunUnready::Unknown(cannot_read(&found, &error))),
        };
        held_by_root(root, &found, &metadata, root_uid).map_err(|reason| {
            RootRunUnready::Untrusted {
                path: PathBuf::from(path),
                reason,
            }
        })?;
        if !is_packaged && !is_same_file(&found, current_helper)? {
            return Err(RootRunUnready::OutOfDate {
                path: PathBuf::from(path),
            });
        }
        return Ok(RootRunHelper {
            path: PathBuf::from(path),
            is_packaged,
        });
    }
    Err(RootRunUnready::NotInstalled)
}

/// Whether `file`, a helper under `root`, is root's alone: a regular file, neither setuid nor
/// setgid, owned by `root_uid` and writable by no group or other user, and so is each folder
/// above it up to `root`, as the kernel resolves them.
fn held_by_root(
    root: &Path,
    file: &Path,
    metadata: &std::fs::Metadata,
    root_uid: u32,
) -> Result<(), String> {
    if !metadata.file_type().is_file() {
        return Err(format!("{} is not a regular file", file.display()));
    }
    if metadata.mode() & 0o6000 != 0 {
        return Err(format!("{} is setuid or setgid", file.display()));
    }
    only_root_writes(file, metadata, root_uid)?;
    let resolve =
        |path: &Path| std::fs::canonicalize(path).map_err(|error| cannot_read(path, &error));
    let top = resolve(root)?;
    let parent = file.parent().unwrap_or(root);
    for folder in resolve(parent)?
        .ancestors()
        .take_while(|folder| folder.starts_with(&top))
    {
        let metadata = std::fs::metadata(folder).map_err(|error| cannot_read(folder, &error))?;
        only_root_writes(folder, &metadata, root_uid)?;
    }
    Ok(())
}

fn only_root_writes(
    path: &Path,
    metadata: &std::fs::Metadata,
    root_uid: u32,
) -> Result<(), String> {
    if metadata.uid() != root_uid {
        return Err(format!(
            "{} is owned by uid {}, not {root_uid}",
            path.display(),
            metadata.uid()
        ));
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(format!(
            "{} is writable by its group or other users (mode {:o})",
            path.display(),
            metadata.mode() & 0o7777
        ));
    }
    Ok(())
}

/// Whether `installed` has `current`'s contents. Sizes first: a helper is megabytes, and the
/// daemon asks at each readiness report.
fn is_same_file(installed: &Path, current: &Path) -> Result<bool, RootRunUnready> {
    let unknown =
        |path: &Path, error: std::io::Error| RootRunUnready::Unknown(cannot_read(path, &error));
    let installed_size = std::fs::metadata(installed)
        .map_err(|error| unknown(installed, error))?
        .len();
    let current_size = std::fs::metadata(current)
        .map_err(|error| unknown(current, error))?
        .len();
    if installed_size != current_size {
        return Ok(false);
    }
    let installed_contents = std::fs::read(installed).map_err(|error| unknown(installed, error))?;
    let current_contents = std::fs::read(current).map_err(|error| unknown(current, error))?;
    Ok(installed_contents == current_contents)
}

fn cannot_read(path: &Path, error: &std::io::Error) -> String {
    format!("cannot read {}: {error}", path.display())
}

/// The root step `torlek setup root-run` runs once with sudo, the helper to copy as its first
/// argument. Running it again replaces the copy with this release's.
pub fn root_run_setup_script() -> String {
    let folder = set_up_folder();
    let parent = set_up_parent();
    format!(
        "{PREAMBLE}\
         helper=$1\n\
         [ ! -e {PACKAGED_ROOT_RUN_HELPER} ] ||\n    \
         fail \"Torlek's package installs the root-run helper already: nothing to set up\"\n\
         [ -f \"$helper\" ] || fail \"no helper at $helper\"\n\
         install -d -o root -g root -m 0755 {parent} {folder}\n\
         install -o root -g root -m 0755 \"$helper\" {SET_UP_ROOT_RUN_HELPER}\n\
         echo \"torlek setup root-run: sudo in approved runs can reach {SET_UP_ROOT_RUN_HELPER}\"\n"
    )
}

/// The root step `torlek setup root-run --uninstall` runs: it removes the setup step's copy
/// alone, and its folders when nothing else is in them.
pub fn root_run_removal_script() -> String {
    let folder = set_up_folder();
    let parent = set_up_parent();
    format!(
        "{PREAMBLE}\
         if [ ! -e {SET_UP_ROOT_RUN_HELPER} ]; then\n    \
         echo \"torlek setup root-run: the setup step installed nothing here: nothing to remove\" >&2\n    \
         exit 0\n\
         fi\n\
         rm -f {SET_UP_ROOT_RUN_HELPER}\n\
         # Only empty folders go: the masks helper's copy shares {parent}.\n\
         rmdir {folder} {parent} 2>/dev/null || true\n\
         echo \"torlek setup root-run: the setup step's root-run helper is removed\"\n"
    )
}

/// `/usr/local/lib/torlek/root-run`.
fn set_up_folder() -> String {
    folder_above(SET_UP_ROOT_RUN_HELPER, 1)
}

/// `/usr/local/lib/torlek`.
fn set_up_parent() -> String {
    folder_above(SET_UP_ROOT_RUN_HELPER, 2)
}

fn folder_above(path: &str, levels: usize) -> String {
    Path::new(path)
        .ancestors()
        .nth(levels)
        .expect("the setup step's helper lies two folders below /")
        .display()
        .to_string()
}

const PREAMBLE: &str = "set -eu\n\
fail() {\n    echo \"torlek setup root-run: $*\" >&2\n    exit 1\n}\n\
[ \"$(id -u)\" = 0 ] || fail \"run it as root\"\n";

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// This process's uid, as the owner of a file it makes.
    fn uid() -> u32 {
        let probe = tempfile::NamedTempFile::new().unwrap();
        probe.as_file().metadata().unwrap().uid()
    }

    /// A host root whose folders and files this test's user owns, mode 0755, as root owns a
    /// host's.
    struct Host {
        root: tempfile::TempDir,
        current_helper: PathBuf,
    }

    impl Host {
        fn new() -> Host {
            let root = tempfile::tempdir().unwrap();
            set_mode(root.path(), 0o755);
            let current_helper = root
                .path()
                .join("home/owner/.local/lib/torlek/bin/torlek-sandbox");
            write(root.path(), &current_helper, "this release's helper");
            Host {
                root,
                current_helper,
            }
        }

        fn at(&self, path: &str) -> PathBuf {
            self.root.path().join(path.trim_start_matches('/'))
        }

        fn install(&self, path: &str, text: &str) {
            write(self.root.path(), &self.at(path), text);
        }

        fn helper(&self) -> Result<RootRunHelper, RootRunUnready> {
            root_run_helper(self.root.path(), &self.current_helper, uid())
        }
    }

    fn write(root: &Path, path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
        set_mode(path, 0o755);
        for folder in path.ancestors().skip(1).take_while(|f| f.starts_with(root)) {
            set_mode(folder, 0o755);
        }
    }

    fn set_mode(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn a_host_without_either_install_has_no_helper() {
        assert_eq!(Host::new().helper(), Err(RootRunUnready::NotInstalled));
    }

    #[test]
    fn a_packages_helper_is_the_helper() {
        let host = Host::new();
        host.install(PACKAGED_ROOT_RUN_HELPER, "the package's helper");

        assert_eq!(
            host.helper(),
            Ok(RootRunHelper {
                path: PathBuf::from(PACKAGED_ROOT_RUN_HELPER),
                is_packaged: true
            })
        );
    }

    #[test]
    fn the_setup_steps_copy_of_this_release_is_the_helper() {
        let host = Host::new();
        host.install(SET_UP_ROOT_RUN_HELPER, "this release's helper");

        assert_eq!(
            host.helper(),
            Ok(RootRunHelper {
                path: PathBuf::from(SET_UP_ROOT_RUN_HELPER),
                is_packaged: false
            })
        );
    }

    #[test]
    fn the_packages_helper_wins_over_the_setup_steps_copy() {
        let host = Host::new();
        host.install(PACKAGED_ROOT_RUN_HELPER, "the package's helper");
        host.install(SET_UP_ROOT_RUN_HELPER, "an earlier release's helper");

        assert!(host.helper().unwrap().is_packaged);
    }

    #[test]
    fn the_setup_steps_copy_of_an_earlier_release_is_out_of_date() {
        let host = Host::new();
        host.install(SET_UP_ROOT_RUN_HELPER, "an earlier release's helper");

        assert_eq!(
            host.helper(),
            Err(RootRunUnready::OutOfDate {
                path: PathBuf::from(SET_UP_ROOT_RUN_HELPER)
            })
        );
    }

    #[test]
    fn a_helper_another_user_can_change_is_not_the_helper() {
        for (changed, mode) in [
            (PACKAGED_ROOT_RUN_HELPER, 0o775),
            ("/usr/lib/torlek/bin", 0o777),
            ("/usr/lib", 0o1777),
        ] {
            let host = Host::new();
            host.install(PACKAGED_ROOT_RUN_HELPER, "the package's helper");
            set_mode(&host.at(changed), mode);

            let Err(RootRunUnready::Untrusted { reason, .. }) = host.helper() else {
                panic!("{changed}: {:?}", host.helper());
            };
            assert!(
                reason.contains("writable by its group"),
                "{changed}: {reason}"
            );
        }
    }

    #[test]
    fn a_helper_root_does_not_own_is_not_the_helper() {
        let host = Host::new();
        host.install(SET_UP_ROOT_RUN_HELPER, "this release's helper");

        let refused = root_run_helper(host.root.path(), &host.current_helper, uid() + 1);

        let Err(RootRunUnready::Untrusted { path, reason }) = refused else {
            panic!("{refused:?}");
        };
        assert_eq!(path, PathBuf::from(SET_UP_ROOT_RUN_HELPER));
        assert!(reason.contains("is owned by uid"), "{reason}");
    }

    #[test]
    fn a_setuid_helper_is_not_the_helper() {
        let host = Host::new();
        host.install(PACKAGED_ROOT_RUN_HELPER, "the package's helper");
        set_mode(&host.at(PACKAGED_ROOT_RUN_HELPER), 0o4755);

        let Err(RootRunUnready::Untrusted { reason, .. }) = host.helper() else {
            panic!("{:?}", host.helper());
        };
        assert!(reason.contains("setuid"), "{reason}");
    }

    #[test]
    fn a_link_at_the_helpers_path_is_not_the_helper() {
        let host = Host::new();
        host.install("/opt/elsewhere/torlek-sandbox", "this release's helper");
        std::fs::create_dir_all(host.at("/usr/local/lib/torlek/root-run")).unwrap();
        std::os::unix::fs::symlink(
            host.at("/opt/elsewhere/torlek-sandbox"),
            host.at(SET_UP_ROOT_RUN_HELPER),
        )
        .unwrap();

        let Err(RootRunUnready::Untrusted { reason, .. }) = host.helper() else {
            panic!("{:?}", host.helper());
        };
        assert!(reason.contains("not a regular file"), "{reason}");
    }

    #[test]
    fn a_folder_reached_through_a_link_is_checked_where_it_leads() {
        let host = Host::new();
        host.install("/opt/torlek/bin/torlek-sandbox", "the package's helper");
        std::fs::create_dir_all(host.at("/usr/lib")).unwrap();
        set_mode(&host.at("/usr"), 0o755);
        set_mode(&host.at("/usr/lib"), 0o755);
        std::os::unix::fs::symlink(host.at("/opt/torlek"), host.at("/usr/lib/torlek")).unwrap();
        set_mode(&host.at("/opt"), 0o777);

        let Err(RootRunUnready::Untrusted { reason, .. }) = host.helper() else {
            panic!("{:?}", host.helper());
        };
        assert!(reason.contains("/opt"), "{reason}");
    }
}
