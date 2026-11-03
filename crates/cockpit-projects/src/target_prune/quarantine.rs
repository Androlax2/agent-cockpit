//! Descriptor-relative effects under a durable guard. This is not protection against
//! hostile same-UID mutators: the owner contract excludes unmanaged concurrent writers.
use super::{filesystem, manifest::Snapshot, mount};
use crate::store::StoredCheckout;
use rustix::fs::{self, AtFlags, Mode, OFlags};
use std::ffi::CString;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

fn name(value: &str) -> Result<CString, String> {
    if !value.starts_with(".torlek-target-prune-") || value.contains('/') {
        return Err("invalid quarantine name".into());
    }
    CString::new(value).map_err(|e| e.to_string())
}

fn root(stored: &StoredCheckout, snapshot: &Snapshot) -> Result<OwnedFd, String> {
    let anchor = crate::storage::open_root(&stored.checkout.path)?;
    crate::storage::verify_marker(stored, &anchor)?;
    if mount::identity(&anchor)?.0 != snapshot.root {
        return Err("registered clone root was replaced".into());
    }
    // The report's O_PATH anchor cannot be fsynced; upgrade through that descriptor.
    let root = fs::openat(
        &anchor,
        c".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|e| e.to_string())?;
    if mount::identity(&root)?.0 != snapshot.root {
        return Err("root anchor changed".into());
    }
    Ok(root)
}

fn sync(fd: &OwnedFd) -> Result<(), String> {
    fs::fsync(fd).map_err(|e| format!("cleanup durability uncertain: {e}"))
}

/// Create only tool-owned metadata. Its identity must be committed BEFORE detachment.
/// A crash before that receipt leaves an untrusted name for owner resolution, not reuse.
pub(super) fn create(
    stored: &StoredCheckout,
    snapshot: &Snapshot,
    quarantine: &str,
) -> Result<String, String> {
    let root = root(stored, snapshot)?;
    let mount = mount::mount_id(&root)?;
    let target = mount::open_child(&root, c"target", true, mount)?;
    if mount::identity(&target)?.0 != snapshot.target {
        return Err("target was replaced".into());
    }
    let name = name(quarantine)?;
    fs::mkdirat(&root, &name, Mode::from_bits_truncate(0o700))
        .map_err(|e| format!("cannot exclusively create quarantine: {e}"))?;
    let directory = mount::open_child(&root, &name, true, mount)?;
    let marker = fs::openat(
        &directory,
        c"operation",
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::from_bits_truncate(0o600),
    )
    .map_err(|e| e.to_string())?;
    {
        use std::io::Write;
        let mut file = std::fs::File::from(marker);
        file.write_all(quarantine.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
    }
    let marker = mount::open_child(&directory, c"operation", false, mount)?;
    let proof = format!(
        "{}|{}",
        mount::identity(&directory)?.0,
        mount::identity(&marker)?.0
    );
    sync(&directory)?;
    sync(&root)?;
    Ok(proof)
}

fn open(root: &OwnedFd, quarantine: &str, proof: &str) -> Result<OwnedFd, String> {
    let (directory_id, marker_id) = proof.split_once('|').ok_or("invalid quarantine proof")?;
    let mount = mount::mount_id(root)?;
    let directory = mount::open_child(root, &name(quarantine)?, true, mount)?;
    let marker = mount::open_child(&directory, c"operation", false, mount)?;
    if mount::identity(&directory)?.0 != directory_id || mount::identity(&marker)?.0 != marker_id {
        return Err("quarantine directory or operation marker was replaced".into());
    }
    Ok(directory)
}

/// Caller has committed detaching intent and revalidated policy, eligibility and manifest.
#[cfg(target_os = "linux")]
pub(super) fn detach(
    stored: &StoredCheckout,
    snapshot: &Snapshot,
    quarantine: &str,
    proof: &str,
) -> Result<(), String> {
    let root = root(stored, snapshot)?;
    let directory = open(&root, quarantine, proof)?;
    let target = mount::open_child(&root, c"target", true, mount::mount_id(&root)?)?;
    if mount::identity(&target)?.0 != snapshot.target {
        return Err("target changed before detachment".into());
    }
    fs::renameat_with(
        &root,
        c"target",
        &directory,
        c"target",
        fs::RenameFlags::NOREPLACE,
    )
    .map_err(|e| format!("target detachment refused: {e}"))?;
    sync(&root)?;
    sync(&directory)?;
    verify_detached(stored, snapshot, quarantine, proof)
}

#[cfg(not(target_os = "linux"))]
pub(super) fn detach(
    _stored: &StoredCheckout,
    _snapshot: &Snapshot,
    _quarantine: &str,
    _proof: &str,
) -> Result<(), String> {
    Err("target pruning requires verified Linux mount-boundary support".into())
}

/// Also used after detachment-before-receipt. A mismatched moved entry is preserved;
/// never automatically restore it over a new target or delete an unapproved manifest.
pub(super) fn verify_detached(
    stored: &StoredCheckout,
    snapshot: &Snapshot,
    quarantine: &str,
    proof: &str,
) -> Result<(), String> {
    let root = root(stored, snapshot)?;
    let directory = open(&root, quarantine, proof)?;
    let target = mount::open_child(&directory, c"target", true, mount::mount_id(&root)?)?;
    if mount::identity(&target)?.0 != snapshot.target {
        return Err("detached target identity differs; owner resolution required".into());
    }
    let observed = filesystem::snapshot_directory(
        target,
        snapshot.root.clone(),
        snapshot.target.clone(),
        mount::mount_id(&root)?,
    )?;
    if &observed != snapshot {
        return Err("detached target contents differ; owner resolution required".into());
    }
    Ok(())
}

pub(super) struct Slice {
    pub cursor: usize,
    pub removed: u64,
    pub absent: u64,
    pub reason: Option<String>,
}

/// A cooperative slice, never detached from its owning Bulk request. Confirmed bytes are
/// logical lengths of successfully unlinked regular files, not reclaimed disk allocation.
pub(super) fn remove_slice(
    stored: &StoredCheckout,
    snapshot: &Snapshot,
    quarantine: &str,
    proof: &str,
    cursor: usize,
) -> Slice {
    let mut result = Slice {
        cursor,
        removed: 0,
        absent: 0,
        reason: None,
    };
    let removed = (|| -> Result<(), String> {
        let root = root(stored, snapshot)?;
        let directory = open(&root, quarantine, proof)?;
        let mount = mount::mount_id(&root)?;
        let until = Instant::now() + Duration::from_millis(100);
        let stop = cursor.saturating_add(1000).min(snapshot.entries.len());
        while result.cursor < stop && Instant::now() < until {
            let entry = &snapshot.entries[snapshot.entries.len() - 1 - result.cursor];
            // Reverse lexical order visits children before their directories; every path
            // component is reopened without links/mount crossings and bound to the plan.
            let Some((parent, name)) = parent(&directory, snapshot, &entry.path, mount)? else {
                result.absent = result
                    .absent
                    .checked_add(entry.bytes)
                    .ok_or("absent-byte overflow")?;
                result.cursor += 1;
                continue;
            };
            let metadata = mount::stat_child(&parent, &name);
            if matches!(metadata, Err(rustix::io::Errno::NOENT)) {
                // May be an interrupted unlink before its receipt: never claim freed bytes.
                result.absent = result
                    .absent
                    .checked_add(entry.bytes)
                    .ok_or("absent-byte overflow")?;
                result.cursor += 1;
                continue;
            }
            metadata.map_err(|e| e.to_string())?;
            let pinned = mount::open_child(&parent, &name, entry.directory, mount)?;
            if mount::identity(&pinned)?.0 != entry.identity {
                return Err("cleanup entry was replaced or changed".into());
            }
            // An unexpected child prevents rmdir; no wildcard/recursive deletion is used.
            fs::unlinkat(
                &parent,
                &name,
                if entry.directory {
                    AtFlags::REMOVEDIR
                } else {
                    AtFlags::empty()
                },
            )
            .map_err(|e| format!("cleanup unlink refused: {e}"))?;
            // If syncing fails, preserve cursor and let recovery classify absence as
            // unconfirmed; a successful unlink alone is not a durable receipt.
            sync(&parent)?;
            result.removed = result
                .removed
                .checked_add(entry.bytes)
                .ok_or("removed-byte overflow")?;
            result.cursor += 1;
        }
        Ok(())
    })();
    result.reason = removed.err();
    result
}

fn parent(
    directory: &OwnedFd,
    snapshot: &Snapshot,
    path: &[u8],
    mount: u64,
) -> Result<Option<(OwnedFd, CString)>, String> {
    if path.is_empty() {
        let parent = fs::openat(
            directory,
            c".",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(|e| e.to_string())?;
        return Ok(Some((parent, c"target".to_owned())));
    }
    if absent(directory, c"target")? {
        return Ok(None);
    }
    let mut parent = mount::open_child(directory, c"target", true, mount)?;
    if mount::identity(&parent)?.0 != snapshot.target {
        return Err("quarantined target was replaced".into());
    }
    let mut parts: Vec<_> = path.split(|b| *b == b'/').collect();
    let last = CString::new(parts.pop().ok_or("empty cleanup path")?).map_err(|e| e.to_string())?;
    let mut prefix = Vec::new();
    for part in parts {
        if !prefix.is_empty() {
            prefix.push(b'/');
        }
        prefix.extend_from_slice(part);
        let index = snapshot
            .entries
            .binary_search_by(|entry| entry.path.cmp(&prefix))
            .map_err(|_| "cleanup parent is absent from manifest")?;
        let expected = &snapshot.entries[index];
        if !expected.directory {
            return Err("cleanup parent is not a directory".into());
        }
        let component = CString::new(part).map_err(|e| e.to_string())?;
        if absent(&parent, &component)? {
            return Ok(None);
        }
        parent = mount::open_child(&parent, &component, true, mount)?;
        if mount::identity(&parent)?.0 != expected.identity {
            return Err("cleanup parent was replaced".into());
        }
    }
    Ok(Some((parent, last)))
}

fn absent(parent: &OwnedFd, name: &std::ffi::CStr) -> Result<bool, String> {
    match mount::stat_child(parent, name) {
        Err(rustix::io::Errno::NOENT) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// Only empty, proved tool metadata is removed. A new root target is never touched.
pub(super) fn finish(
    stored: &StoredCheckout,
    snapshot: &Snapshot,
    quarantine: &str,
    proof: &str,
) -> Result<(), String> {
    let root = root(stored, snapshot)?;
    let name = name(quarantine)?;
    // This phase is entered only after every manifest entry has a durable receipt.
    // Metadata may already have been removed before the finishing receipt committed.
    if absent(&root, &name)? {
        return sync(&root);
    }
    let (directory_id, marker_id) = proof.split_once('|').ok_or("invalid quarantine proof")?;
    let mount = mount::mount_id(&root)?;
    let directory = mount::open_child(&root, &name, true, mount)?;
    if mount::identity(&directory)?.0 != directory_id {
        return Err("quarantine was replaced before finishing".into());
    }
    if !absent(&directory, c"target")? {
        return Err("quarantined target is not confirmed absent".into());
    }
    if !absent(&directory, c"operation")? {
        let marker = mount::open_child(&directory, c"operation", false, mount)?;
        if mount::identity(&marker)?.0 != marker_id {
            return Err("quarantine marker was replaced before finishing".into());
        }
        fs::unlinkat(&directory, c"operation", AtFlags::empty()).map_err(|e| e.to_string())?;
    }
    sync(&directory)?;
    fs::unlinkat(&root, &name, AtFlags::REMOVEDIR).map_err(|e| e.to_string())?;
    sync(&root)
}

#[cfg(all(test, target_os = "linux"))]
#[path = "quarantine_tests.rs"]
mod tests;
