//! `cockpit support-bundle`: one size-bounded tar archive of Cockpit's own logs, the sections
//! other owners provide (version manifest, readiness, redacted config, schema version) and
//! platform facts. [`plan_bundle`] reads everything first so the caller can list the exact
//! contents before [`BundlePlan::write`] creates the archive.

use std::fs::{self, File};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::Error;
use crate::error::{check_plain_name, io_error};
use crate::rotation::rotation_number;

/// Logs are capped at 16 MiB per process; this leaves room for a few processes and sections.
pub const DEFAULT_BUNDLE_MAX_BYTES: u64 = 64 * 1024 * 1024;

const ROOT: &str = "cockpit-support-bundle";
const PLATFORM_FACTS: &str = "platform.json";
const LOGS: &str = "logs";
const TAR_BLOCK: u64 = 512;

#[derive(Debug, Clone)]
pub struct BundleRequest {
    /// Per-process log directories, as [`crate::log_dir_in`] returns them.
    pub log_dirs: Vec<PathBuf>,
    /// Leaves out log files last written before this time (`--since`).
    pub since: Option<SystemTime>,
    pub sections: Vec<BundleSection>,
    /// Bound on the whole archive, tar headers included.
    pub max_bytes: u64,
}

impl BundleRequest {
    pub fn new(log_dirs: Vec<PathBuf>) -> Self {
        Self {
            log_dirs,
            since: None,
            sections: Vec::new(),
            max_bytes: DEFAULT_BUNDLE_MAX_BYTES,
        }
    }
}

/// A file another owner contributes. Its owner redacts it: this crate copies bytes as given.
#[derive(Debug, Clone)]
pub struct BundleSection {
    name: String,
    contents: Vec<u8>,
}

impl BundleSection {
    /// `name` is a plain file name, not `logs` or `platform.json`, which this crate writes.
    pub fn new(name: &str, contents: Vec<u8>) -> Result<Self, Error> {
        check_plain_name("bundle section", name)?;
        if name == LOGS || name == PLATFORM_FACTS {
            return Err(Error::InvalidName {
                kind: "bundle section",
                name: name.to_owned(),
            });
        }
        Ok(Self {
            name: name.to_owned(),
            contents,
        })
    }
}

/// One archive entry as listed before writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleItem {
    pub path: String,
    pub bytes: u64,
}

#[derive(Debug)]
pub struct BundlePlan {
    items: Vec<BundleItem>,
    contents: Vec<Vec<u8>>,
}

pub fn plan_bundle(request: &BundleRequest) -> Result<BundlePlan, Error> {
    let mut plan = BundlePlan {
        items: Vec::new(),
        contents: Vec::new(),
    };
    for log_dir in &request.log_dirs {
        let process = log_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        check_plain_name("log directory", &process)?;
        for (file_name, contents) in read_logs(log_dir, request.since)? {
            plan.push(format!("{LOGS}/{process}/{file_name}"), contents)?;
        }
    }
    for section in &request.sections {
        plan.push(section.name.clone(), section.contents.clone())?;
    }
    plan.push(String::from(PLATFORM_FACTS), platform_facts())?;
    let bytes = plan.archive_bytes();
    if bytes > request.max_bytes {
        return Err(Error::BundleTooLarge {
            bytes,
            max: request.max_bytes,
        });
    }
    Ok(plan)
}

impl BundlePlan {
    pub fn contents(&self) -> &[BundleItem] {
        &self.items
    }

    /// The contents of `name`, a path inside the bundle such as `platform.json`.
    pub fn entry(&self, name: &str) -> Option<&[u8]> {
        let path = archive_path(name);
        let index = self.items.iter().position(|item| item.path == path)?;
        Some(&self.contents[index])
    }

    /// The archive's exact size: a header block per entry, contents padded to whole blocks,
    /// and the two zero blocks that end a tar archive.
    pub fn archive_bytes(&self) -> u64 {
        let entries: u64 = self
            .items
            .iter()
            .map(|item| TAR_BLOCK + item.bytes.next_multiple_of(TAR_BLOCK))
            .sum();
        entries + 2 * TAR_BLOCK
    }

    /// Writes the archive to a new file, 0600. An existing file is never replaced, and a
    /// partial archive is removed when writing fails.
    pub fn write(&self, destination: &Path) -> Result<(), Error> {
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(destination)
            .map_err(io_error("create the support bundle", destination))?;
        let written = self.write_tar(&mut file).and_then(|()| file.sync_all());
        if let Err(source) = written {
            drop(file);
            // The write error is the one to report; a failed cleanup leaves a visible partial file.
            let _ = fs::remove_file(destination);
            return Err(io_error("write the support bundle", destination)(source));
        }
        Ok(())
    }

    fn push(&mut self, name: String, contents: Vec<u8>) -> Result<(), Error> {
        let path = archive_path(&name);
        if self.items.iter().any(|item| item.path == path) {
            return Err(Error::InvalidName {
                kind: "duplicate bundle entry",
                name,
            });
        }
        if path.len() > 100 {
            return Err(Error::InvalidName {
                kind: "bundle entry longer than 100 bytes",
                name,
            });
        }
        self.items.push(BundleItem {
            path,
            bytes: contents.len() as u64,
        });
        self.contents.push(contents);
        Ok(())
    }

    fn write_tar(&self, archive: &mut impl Write) -> io::Result<()> {
        let modified = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        for (item, contents) in self.items.iter().zip(&self.contents) {
            archive.write_all(&tar_header(&item.path, item.bytes, modified))?;
            archive.write_all(contents)?;
            let padding = item.bytes.next_multiple_of(TAR_BLOCK) - item.bytes;
            archive.write_all(&[0; TAR_BLOCK as usize][..padding as usize])?;
        }
        archive.write_all(&[0; 2 * TAR_BLOCK as usize])
    }
}

fn archive_path(name: &str) -> String {
    format!("{ROOT}/{name}")
}

/// `cockpit.log` and its rotations, oldest number last, skipping files outside the window.
fn read_logs(log_dir: &Path, since: Option<SystemTime>) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let mut logs = Vec::new();
    for entry in fs::read_dir(log_dir).map_err(io_error("read the log directory", log_dir))? {
        let entry = entry.map_err(io_error("read the log directory", log_dir))?;
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = rotation_number(&file_name) else {
            continue;
        };
        let path = entry.path();
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .map_err(io_error("read", &path))?;
        if since.is_some_and(|since| modified < since) {
            continue;
        }
        logs.push((
            number,
            file_name,
            fs::read(&path).map_err(io_error("read", &path))?,
        ));
    }
    logs.sort_by_key(|(number, ..)| *number);
    Ok(logs
        .into_iter()
        .map(|(_, file_name, contents)| (file_name, contents))
        .collect())
}

/// Facts about the machine, never about its environment variables or users.
fn platform_facts() -> Vec<u8> {
    let kernel_release = fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let facts = serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "wsl": kernel_release.to_ascii_lowercase().contains("microsoft"),
        "cockpit_diagnostics_version": env!("CARGO_PKG_VERSION"),
    });
    let mut contents = serde_json::to_vec_pretty(&facts).expect("JSON values always serialize");
    contents.push(b'\n');
    contents
}

/// A POSIX ustar header for a regular 0600 file; `path` is at most 100 bytes.
fn tar_header(path: &str, size: u64, modified: u64) -> [u8; TAR_BLOCK as usize] {
    let mut header = [0u8; TAR_BLOCK as usize];
    let mut put =
        |offset: usize, value: &[u8]| header[offset..offset + value.len()].copy_from_slice(value);
    put(0, path.as_bytes());
    put(100, b"0000600\0");
    put(108, b"0000000\0");
    put(116, b"0000000\0");
    put(124, format!("{size:011o}\0").as_bytes());
    put(136, format!("{modified:011o}\0").as_bytes());
    put(148, b"        ");
    put(156, b"0");
    put(257, b"ustar\0");
    put(263, b"00");
    let checksum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
    header[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    header
}
