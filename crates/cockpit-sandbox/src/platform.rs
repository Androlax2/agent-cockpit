//! Which platform the daemon runs on, as far as the sandbox cares (report 11): Linux, macOS,
//! or Linux inside WSL2, where Windows interop reaches past any Linux sandbox.

use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Platform {
    Linux,
    MacOs,
    /// Linux in a WSL2 distribution. `windows_drives` are the mounted Windows drives
    /// (`/mnt/c`...); `interop_sockets` the folder whose sockets launch Windows programs.
    Wsl2 {
        windows_drives: Vec<PathBuf>,
        interop_sockets: PathBuf,
    },
    /// Anything else, by its `std::env::consts::OS` name: no backend enforces a profile there.
    Unsupported(String),
}

/// WSL2 puts its interop sockets here; denying the folder keeps Windows programs out of
/// reach even when a Windows path is known.
const WSL_INTEROP_SOCKETS: &str = "/run/WSL";

impl Platform {
    /// This host's platform: the OS Rust was built for, and on Linux whether the kernel is
    /// WSL2's (its `/proc/version` names Microsoft).
    pub fn detect() -> Platform {
        match std::env::consts::OS {
            "macos" => Platform::MacOs,
            "linux" => {
                let kernel = std::fs::read_to_string("/proc/version").unwrap_or_default();
                if is_wsl_kernel(&kernel) {
                    Platform::Wsl2 {
                        windows_drives: windows_drives_in(Path::new("/mnt")),
                        interop_sockets: PathBuf::from(WSL_INTEROP_SOCKETS),
                    }
                } else {
                    Platform::Linux
                }
            }
            other => Platform::Unsupported(other.to_owned()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Platform::Linux => "linux",
            Platform::MacOs => "macos",
            Platform::Wsl2 { .. } => "wsl2",
            Platform::Unsupported(name) => name,
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// WSL2 kernels say `microsoft` in their version string (`...-microsoft-standard-WSL2`).
pub(crate) fn is_wsl_kernel(proc_version: &str) -> bool {
    proc_version.to_ascii_lowercase().contains("microsoft")
}

/// The one-letter folders under `mount_root`, where WSL mounts Windows drives.
pub(crate) fn windows_drives_in(mount_root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(mount_root) else {
        return Vec::new();
    };
    let mut drives: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.len() == 1 && name.chars().all(|letter| letter.is_ascii_lowercase())
        })
        .map(|entry| entry.path())
        .collect();
    drives.sort();
    drives
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wsl2_kernel_is_recognized_by_its_version_string() {
        assert!(is_wsl_kernel(
            "Linux version 5.15.153.1-microsoft-standard-WSL2 (root@...) #1 SMP"
        ));
        assert!(!is_wsl_kernel(
            "Linux version 7.2.11-arch1-1 (linux@archlinux)"
        ));
    }

    #[test]
    fn windows_drives_are_the_one_letter_mounts() {
        let mounts = tempfile::tempdir().unwrap();
        for name in ["c", "d", "wsl", "C1"] {
            std::fs::create_dir(mounts.path().join(name)).unwrap();
        }

        assert_eq!(
            windows_drives_in(mounts.path()),
            vec![mounts.path().join("c"), mounts.path().join("d")]
        );
    }
}
