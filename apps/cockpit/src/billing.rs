//! Metadata-only evidence captured while requests run, separate from their token-price estimate.

use crate::{costs::Billing, paths, util};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Deserialize;
use std::fs;
use std::io;
use std::path::Path;

const MAX_RECORD_BYTES: u64 = 4096;

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

fn file_stamp(path: &Path) -> io::Result<(f64, i64)> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() { return Err(io::Error::other("not a regular file")); }
    let modified = metadata.modified()?.duration_since(std::time::UNIX_EPOCH).map_err(io::Error::other)?.as_secs_f64();
    Ok((modified, metadata.len() as i64))
}

/// Imports only changed metadata files; errors are reported without hiding token usage.
pub fn import(archive: &mut Connection) -> io::Result<usize> {
    let directory = paths::billing_dir();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    if fs::symlink_metadata(&directory)?.file_type().is_symlink() {
        return Err(io::Error::other("billing directory must not be a symlink"));
    }
    let transaction = archive.transaction().map_err(io::Error::other)?;
    let mut changed = 0;
    let mut errors = Vec::new();
    for entry in entries {
        let path = match entry { Ok(entry) => entry.path(), Err(error) => { errors.push(error.to_string()); continue; } };
        if path.extension().is_none_or(|extension| extension != "json") { continue; }
        let (modified, size) = match file_stamp(&path) {
            Ok(stamp) => stamp,
            Err(error) => { errors.push(format!("{}: {error}", path.display())); continue; }
        };
        let name = path.to_string_lossy();
        let known = transaction.query_row("select mtime, size from billing_files where path = ?", [name.as_ref()],
            |row| Ok((row.get::<_, f64>(0)?, row.get::<_, i64>(1)?))).optional().map_err(io::Error::other)?;
        if known == Some((modified, size)) { continue; }
        let record = match util::read_untrusted(&path, MAX_RECORD_BYTES + 1).and_then(|bytes| decode(&bytes)) {
            Ok(record) => record,
            Err(error) => { errors.push(format!("{}: {error}", path.display())); continue; }
        };
        let mode = match record.billing { Billing::Api => "api", Billing::Subscription => "subscription", Billing::Unknown => "unknown" };
        changed += transaction.execute("insert into reply_billing values (?, ?) on conflict(key) do update
            set billing = 'unknown' where reply_billing.billing != excluded.billing", params![record.key, mode]).map_err(io::Error::other)?;
        transaction.execute("insert or replace into billing_files values (?, ?, ?)", params![name.as_ref(), modified, size]).map_err(io::Error::other)?;
    }
    transaction.commit().map_err(io::Error::other)?;
    if !errors.is_empty() { return Err(io::Error::other(format!("billing metadata: {}", errors.join("; ")))); }
    Ok(changed)
}
