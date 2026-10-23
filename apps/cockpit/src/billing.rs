//! Metadata-only evidence captured while requests run, separate from their token-price estimate.

use crate::{costs::Billing, paths, util};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

const MAX_RECORD_BYTES: u64 = 4096;
const MAX_PENDING_ENTRIES: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    key: String,
    billing: Billing,
}

fn decode(bytes: &[u8]) -> io::Result<Evidence> {
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(io::Error::other("billing record exceeds 4096 bytes"));
    }
    let record: Evidence = serde_json::from_slice(bytes)?;
    if record.key.is_empty() || record.key.len() > 512 || !record.key.bytes().all(|byte| byte.is_ascii_alphanumeric() || b":_-".contains(&byte)) {
        return Err(io::Error::other("billing reply key is empty or invalid"));
    }
    Ok(record)
}

fn open_directory(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path)
}

fn directory_path(directory: &fs::File) -> PathBuf {
    // Anchor every operation to the opened directory, even if a sandbox replaces its name.
    PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()))
}

fn record_evidence(archive: &Connection, record: Evidence) -> rusqlite::Result<usize> {
    let mode = match record.billing { Billing::Api => "api", Billing::Subscription => "subscription", Billing::Unknown => "unknown" };
    archive.execute("insert into reply_billing values (?, ?) on conflict(key) do update
        set billing = 'unknown' where reply_billing.billing != excluded.billing", params![record.key, mode])
}

fn retain(source: &Path, destination: &Path) -> io::Result<()> {
    match fs::symlink_metadata(destination) {
        Ok(_) => {
            let original = util::read_untrusted(source, MAX_RECORD_BYTES + 1)?;
            if util::read_untrusted(destination, MAX_RECORD_BYTES + 1)? != original {
                return Err(io::Error::other("retained billing record conflicts with its filename"));
            }
            fs::remove_file(source)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => fs::rename(source, destination),
        Err(error) => Err(error),
    }
}

/// Claims at most one pending batch; committed evidence remains replayable on disk.
pub fn import(archive: &mut Connection) -> io::Result<usize> {
    let directory = match open_directory(&paths::billing_dir()) {
        Ok(directory) => directory,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    // Keep the lock through post-commit moves; another importer must not claim those files again.
    if unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = io::Error::last_os_error();
        return if error.kind() == io::ErrorKind::WouldBlock { Ok(0) } else { Err(error) };
    }
    let pending = directory_path(&directory);
    let mut folders = Vec::new();
    for name in ["processing", "processed", "rejected"] {
        let path = pending.join(name);
        match fs::create_dir(&path) {
            Ok(()) => {},
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error),
        }
        folders.push(open_directory(&path)?);
    }
    let processing = directory_path(&folders[0]);
    let processed = directory_path(&folders[1]);
    let rejected = directory_path(&folders[2]);
    let transaction = archive.transaction_with_behavior(TransactionBehavior::Immediate).map_err(io::Error::other)?;
    transaction.execute_batch("create table if not exists billing_import_state (id integer primary key check (id = 1));").map_err(io::Error::other)?;
    let mut changed = 0;
    let mut errors = Vec::new();
    let mut completed = Vec::new();
    let recovered: bool = transaction.query_row("select exists(select 1 from billing_import_state)", [], |row| row.get(0)).map_err(io::Error::other)?;
    if !recovered {
        // A fresh archive pays for one full replay; ordinary refreshes never enumerate retained history.
        for entry in fs::read_dir(&processed)? {
            let path = entry?.path();
            match util::read_untrusted(&path, MAX_RECORD_BYTES + 1).and_then(|bytes| decode(&bytes)) {
                Ok(record) => changed += record_evidence(&transaction, record).map_err(io::Error::other)?,
                Err(error) => {
                    errors.push(format!("{}: {error}", path.display()));
                    completed.push((path, rejected.join(format!("{}.json", util::token_hex(16)))));
                }
            }
        }
        transaction.execute("insert into billing_import_state values (1)", []).map_err(io::Error::other)?;
    }
    let mut remaining = MAX_PENDING_ENTRIES;
    for source in [&processing, &pending] {
        for entry in fs::read_dir(source)?.take(remaining) {
            remaining -= 1;
            let path = match entry { Ok(entry) => entry.path(), Err(error) => { errors.push(error.to_string()); continue; } };
            if path.extension().is_none_or(|extension| extension != "json") { continue; }
            let claimed = if source == &pending {
                let claimed = processing.join(format!("{}.json", util::token_hex(16)));
                if let Err(error) = fs::rename(&path, &claimed) {
                    errors.push(format!("{}: {error}", path.display()));
                    continue;
                }
                claimed
            } else { path };
            let bytes = match util::read_untrusted(&claimed, MAX_RECORD_BYTES + 1) {
                Ok(bytes) => bytes,
                Err(error) => {
                    errors.push(format!("{}: {error}", claimed.display()));
                    completed.push((claimed, rejected.join(format!("{}.json", util::token_hex(16)))));
                    continue;
                }
            };
            match decode(&bytes) {
                Ok(record) => {
                    changed += record_evidence(&transaction, record).map_err(io::Error::other)?;
                    completed.push((claimed, processed.join(format!("{:x}.json", Sha256::digest(&bytes)))));
                }
                Err(error) => {
                    errors.push(format!("{}: {error}", claimed.display()));
                    completed.push((claimed, rejected.join(format!("{}.json", util::token_hex(16)))));
                }
            }
        }
    }
    transaction.commit().map_err(io::Error::other)?;
    for (source, destination) in completed {
        if let Err(error) = retain(&source, &destination) {
            errors.push(format!("{}: {error}", source.display()));
        }
    }
    if !errors.is_empty() { return Err(io::Error::other(format!("billing metadata: {}", errors.join("; ")))); }
    Ok(changed)
}
