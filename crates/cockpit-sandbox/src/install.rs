//! Verified, Cockpit-owned engine releases. No PATH lookup or local compilation.

use std::collections::BTreeSet;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};

use crate::{GreywallPrograms, parse_greyproxy_version, parse_greywall_version};

const MAX_ARCHIVE: u64 = 128 * 1024 * 1024;
const MAX_EXPANDED: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 32;
const DOWNLOAD_DEADLINE: Duration = Duration::from_secs(100);

/// Failures keep their category through the CLI, without weakening a pin or policy.
#[derive(Debug)]
pub enum InstallError {
    UnsupportedTarget(String),
    Pins(String),
    Download(String),
    Checksum {
        engine: &'static str,
        expected: String,
        actual: String,
    },
    Archive {
        engine: &'static str,
        reason: String,
    },
    Commit {
        engine: &'static str,
        expected: String,
        actual: String,
    },
    UnsafePath(PathBuf),
    ExistingInstallation(PathBuf),
    Io(io::Error),
}

impl InstallError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedTarget(_) => "unsupported-target",
            Self::Pins(_) => "invalid-pins",
            Self::Download(_) => "download",
            Self::Checksum { .. } => "checksum-mismatch",
            Self::Archive { .. } => "invalid-archive",
            Self::Commit { .. } => "commit-mismatch",
            Self::UnsafePath(_) => "unsafe-path",
            Self::ExistingInstallation(_) => "changed-installation",
            Self::Io(_) => "io",
        }
    }
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: ", self.code())?;
        match self {
            Self::UnsupportedTarget(target) => write!(f, "no pinned engines for {target}"),
            Self::Pins(reason) | Self::Download(reason) => f.write_str(reason),
            Self::Checksum {
                engine,
                expected,
                actual,
            } => write!(
                f,
                "{engine}: expected SHA-256 {expected}, got {actual}; nothing activated"
            ),
            Self::Archive { engine, reason } => write!(f, "{engine}: {reason}"),
            Self::Commit {
                engine,
                expected,
                actual,
            } => write!(f, "{engine}: expected commit {expected}, got {actual}"),
            Self::UnsafePath(path) => write!(
                f,
                "{} is not a real, owner-private directory",
                path.display()
            ),
            Self::ExistingInstallation(path) => write!(
                f,
                "{} differs from its verified release; inspect and remove that installation before retrying",
                path.display()
            ),
            Self::Io(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for InstallError {}
impl From<io::Error> for InstallError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Clone)]
struct Pin {
    name: &'static str,
    commit: String,
    url: String,
    sha256: String,
    archive_root: String,
}

/// WSL2 uses its Linux architecture. Unsupported platforms never select a substitute asset.
pub fn host_target() -> Result<&'static str, InstallError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-unknown-linux-gnu"),
        ("linux", "aarch64") => Ok("aarch64-unknown-linux-gnu"),
        ("macos", "aarch64") => Ok("aarch64-apple-darwin"),
        (os, arch) => Err(InstallError::UnsupportedTarget(format!("{arch}-{os}"))),
    }
}

fn pins_for(target: &str) -> Result<[Pin; 2], InstallError> {
    let manifest: serde_json::Value = serde_json::from_str(include_str!("../engine-pins.json"))
        .map_err(|error| InstallError::Pins(error.to_string()))?;
    let pin = |name: &'static str| -> Result<Pin, InstallError> {
        let engine = &manifest[name];
        let asset = engine["assets"]
            .get(target)
            .ok_or_else(|| InstallError::UnsupportedTarget(target.into()))?;
        let string = |value: &serde_json::Value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| InstallError::Pins(format!("{name}: missing string")))
        };
        let commit = string(&engine["commit"])?;
        let tag = string(&engine["tag"])?;
        let url = string(&asset["url"])?;
        let sha256 = string(&asset["sha256"])?;
        let prefix = format!("https://github.com/Androlax2/{name}/releases/download/{tag}/");
        let filename = url
            .strip_prefix(&prefix)
            .and_then(|s| s.strip_suffix(".tar.gz"))
            .filter(|s| {
                !s.is_empty()
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            })
            .ok_or_else(|| InstallError::Pins(format!("{name}: invalid release asset URL")))?;
        if !hex(&commit, 40) || !hex(&sha256, 64) {
            return Err(InstallError::Pins(format!(
                "{name}: invalid commit or SHA-256"
            )));
        }
        let archive_root = if name == "greywall" {
            filename.to_owned()
        } else {
            String::new()
        };
        Ok(Pin {
            name,
            commit,
            url,
            sha256,
            archive_root,
        })
    };
    Ok([pin("greywall")?, pin("greyproxy")?])
}

fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn release_dir(state: &Path, pins: &[Pin; 2]) -> PathBuf {
    state
        .join("engines")
        .join(format!("{}-{}", pins[0].sha256, pins[1].sha256))
}

fn paths(release: &Path, pins: &[Pin; 2]) -> GreywallPrograms {
    GreywallPrograms {
        greywall: release
            .join("greywall")
            .join(&pins[0].archive_root)
            .join("greywall"),
        greyproxy: release.join("greyproxy/greyproxy"),
    }
}

/// The paths this release owns, even before installation. No mutable "current" link:
/// an older Cockpit and its sessions retain their own exact pair during an upgrade.
pub fn programs(state: &Path) -> Result<GreywallPrograms, InstallError> {
    let pins = pins_for(host_target()?)?;
    Ok(paths(&release_dir(state, &pins), &pins))
}

/// Installs the active release into `<profile state>/engines/<asset hashes>`.
/// Both assets must verify before extraction; the complete pair becomes visible in one rename.
pub fn install(state: &Path) -> Result<GreywallPrograms, InstallError> {
    let pins = pins_for(host_target()?)?;
    install_with(state, &pins, download)
}

fn private_dir(path: &Path) -> Result<(), InstallError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
        return Err(InstallError::UnsafePath(path.into()));
    }
    Ok(())
}

fn install_with(
    state: &Path,
    pins: &[Pin; 2],
    fetch: impl Fn(&Pin, &Path) -> Result<(), InstallError>,
) -> Result<GreywallPrograms, InstallError> {
    private_dir(state)?;
    let root = state.join("engines");
    private_dir(&root)?;
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&root)?;
    // All downloads and checksums precede every extracted executable, even for engine two.
    for pin in pins {
        let archive = staging.path().join(format!("{}.tar.gz", pin.name));
        fetch(pin, &archive)?;
        let actual = digest(&archive, MAX_ARCHIVE)?;
        if actual != pin.sha256 {
            return Err(InstallError::Checksum {
                engine: pin.name,
                expected: pin.sha256.clone(),
                actual,
            });
        }
    }
    let complete = staging.path().join("complete");
    private_dir(&complete)?;
    for pin in pins {
        let destination = complete.join(pin.name);
        private_dir(&destination)?;
        extract(
            &staging.path().join(format!("{}.tar.gz", pin.name)),
            &destination,
            pin,
        )?;
    }
    let programs = paths(&complete, pins);
    for (pin, program) in pins.iter().zip([&programs.greywall, &programs.greyproxy]) {
        let output = crate::greywall::program_output(program, "--version").map_err(|reason| {
            InstallError::Archive {
                engine: pin.name,
                reason,
            }
        })?;
        let build = if pin.name == "greywall" {
            parse_greywall_version(&output).map(|(build, _)| build)
        } else {
            parse_greyproxy_version(&output)
        }
        .map_err(|reason| InstallError::Archive {
            engine: pin.name,
            reason,
        })?;
        if build.commit != pin.commit {
            return Err(InstallError::Commit {
                engine: pin.name,
                expected: pin.commit.clone(),
                actual: build.commit,
            });
        }
    }
    let release = release_dir(state, pins);
    match fs::rename(&complete, &release) {
        Ok(()) => File::open(&root)?.sync_all()?,
        Err(error) if release.exists() => {
            // Concurrent installs of the same pair and reruns are harmless; altered installs
            // are never silently accepted or overwritten while a session might use them.
            if !same_tree(&complete, &release)? {
                return Err(InstallError::ExistingInstallation(release));
            }
            let _ = error;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(paths(&release, pins))
}

fn download(pin: &Pin, path: &Path) -> Result<(), InstallError> {
    let mut child = Command::new("curl")
        .args([
            "-q",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "15",
            "--max-time",
            "90",
            "--max-filesize",
        ])
        .arg(MAX_ARCHIVE.to_string())
        .arg("--output")
        .arg(path)
        .arg(&pin.url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| InstallError::Download(format!("curl for {}: {error}", pin.name)))?;
    let deadline = Instant::now() + DOWNLOAD_DEADLINE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(InstallError::Download(format!(
                    "{}: curl exited {status}",
                    pin.name
                )));
            }
            Ok(None)
                if Instant::now() < deadline
                    && fs::metadata(path).map_or(true, |m| m.len() <= MAX_ARCHIVE) =>
            {
                std::thread::sleep(Duration::from_millis(20))
            }
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(InstallError::Download(format!(
                    "{}: download exceeded its time/size bound or could not be waited for ({result:?})",
                    pin.name
                )));
            }
        }
    }
}

fn digest(path: &Path, limit: u64) -> Result<String, InstallError> {
    let file = File::open(path)?;
    if !file.metadata()?.is_file() || file.metadata()?.len() > limit {
        return Err(io::Error::other(format!("{} exceeds the file bound", path.display())).into());
    }
    let mut bounded = file.take(limit + 1);
    let mut hash = Sha256::new();
    let count = io::copy(&mut bounded, &mut hash)?;
    if count > limit {
        return Err(io::Error::other("file grew beyond its bound").into());
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn extract(archive: &Path, destination: &Path, pin: &Pin) -> Result<(), InstallError> {
    let invalid = |reason: String| InstallError::Archive {
        engine: pin.name,
        reason,
    };
    let decoded = GzDecoder::new(File::open(archive)?).take(MAX_EXPANDED + 1);
    let mut archive = tar::Archive::new(decoded);
    let mut seen = BTreeSet::new();
    let entries = archive.entries().map_err(|e| invalid(e.to_string()))?;
    for entry in entries.raw(true) {
        let mut entry = entry.map_err(|e| invalid(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| invalid(e.to_string()))?
            .into_owned();
        if path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
            || path.as_os_str().is_empty()
        {
            return Err(invalid(format!("unsafe member {}", path.display())));
        }
        let relative = if pin.archive_root.is_empty() {
            path.as_path()
        } else {
            path.strip_prefix(&pin.archive_root)
                .map_err(|_| invalid(format!("wrong archive root: {}", path.display())))?
        };
        let relative = relative
            .to_str()
            .ok_or_else(|| invalid("non-UTF-8 member".into()))?;
        if !seen.insert(relative.to_owned()) || seen.len() > MAX_ENTRIES {
            return Err(invalid("duplicate member or too many entries".into()));
        }
        let kind = entry.header().entry_type();
        let target = destination.join(&path);
        let is_directory =
            relative.is_empty() || (pin.name == "greywall" && relative == "LICENSES");
        if is_directory && kind.is_dir() && entry.size() == 0 {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&target)?;
            continue;
        }
        if pin.name == "greywall"
            && relative == "greywatch"
            && kind.is_symlink()
            && entry.size() == 0
        {
            if entry
                .link_name()
                .map_err(|e| invalid(e.to_string()))?
                .as_deref()
                != Some(Path::new("greywall"))
            {
                return Err(invalid(
                    "greywatch must link to greywall in its own directory".into(),
                ));
            }
            // Create the sole allowed link after all file writes and integrity checks.
            continue;
        }
        let allowed = relative == pin.name
            || relative == "LICENSE"
            || (pin.name == "greywall"
                && matches!(relative, "NOTICE" | "LICENSES/tun2socks-GPL-3.0.txt"));
        if !allowed || !kind.is_file() || entry.size() == 0 || entry.size() > MAX_ARCHIVE {
            return Err(invalid(format!(
                "unexpected member/type/size: {}",
                path.display()
            )));
        }
        let parent = target.parent().expect("archive file has a destination");
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let mut file = File::create_new(&target)?;
        let size = io::copy(&mut entry, &mut file).map_err(|e| invalid(e.to_string()))?;
        if size != entry.size() {
            return Err(invalid("truncated member".into()));
        }
        file.set_permissions(fs::Permissions::from_mode(if relative == pin.name {
            0o755
        } else {
            0o644
        }))?;
        file.flush()?;
        file.sync_all()?;
    }
    // Read through the gzip trailer: tar's end marker alone does not validate CRC or size.
    let mut decoded = archive.into_inner();
    io::copy(&mut decoded, &mut io::sink()).map_err(|e| invalid(e.to_string()))?;
    if decoded.limit() == 0 {
        return Err(invalid("expanded archive exceeds its bound".into()));
    }
    let mut required = vec![pin.name, "LICENSE"];
    if pin.name == "greywall" {
        required.extend(["NOTICE", "LICENSES/tun2socks-GPL-3.0.txt", "greywatch"]);
    }
    for name in required {
        if !seen.contains(name) {
            return Err(invalid(format!("missing {name}")));
        }
    }
    if pin.name == "greywall" {
        std::os::unix::fs::symlink(
            "greywall",
            destination.join(&pin.archive_root).join("greywatch"),
        )?;
    }
    Ok(())
}

fn same_tree(expected: &Path, actual: &Path) -> Result<bool, InstallError> {
    let Ok(metadata) = fs::symlink_metadata(actual) else {
        return Ok(false);
    };
    let want = fs::symlink_metadata(expected)?;
    if metadata.file_type() != want.file_type() {
        return Ok(false);
    }
    if want.is_symlink() {
        return Ok(fs::read_link(expected)? == fs::read_link(actual)?);
    }
    if want.permissions().mode() & 0o7777 != metadata.permissions().mode() & 0o7777 {
        return Ok(false);
    }
    if want.is_file() {
        return Ok(digest(expected, MAX_ARCHIVE)? == digest(actual, MAX_ARCHIVE)?);
    }
    let names = |dir: &Path| -> io::Result<BTreeSet<std::ffi::OsString>> {
        fs::read_dir(dir)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect()
    };
    let expected_names = names(expected)?;
    if expected_names != names(actual)? {
        return Ok(false);
    }
    for name in expected_names {
        if !same_tree(&expected.join(&name), &actual.join(&name))? {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
