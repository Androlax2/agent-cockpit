//! Read-only manifest and local ancestry proof. No path-based recursive deletion.
use super::{
    PruneEvidence, PrunePolicy, PrunePreview,
    manifest::{self, Entry, Snapshot},
    mount,
};
use crate::store::StoredCheckout;
use crate::{CheckoutKind, CheckoutState, Git, TargetSize};
use rustix::fs::{self, AtFlags, Dir};
use std::ffi::{CString, OsStr};
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};

pub(super) fn preview(
    stored: StoredCheckout,
    main: StoredCheckout,
    git: &Git,
    policy: PrunePolicy,
    evidence: PruneEvidence,
) -> PrunePreview {
    let mut result = PrunePreview {
        checkout: stored.checkout.clone(),
        measurement: TargetSize::Missing,
        eligibility: None,
        kept_reason: None,
        confirmation_required: true,
        snapshot: None,
        policy,
    };
    if matches!(stored.checkout.kind, CheckoutKind::Main)
        || stored.checkout.state != CheckoutState::Live
    {
        result.kept_reason = Some("only live registered clones are candidates".to_owned());
        return result;
    }
    match observe(&stored, b"target") {
        Ok(Some(snapshot)) => {
            result.measurement = TargetSize::Complete {
                apparent_bytes: snapshot.bytes,
            };
            result.snapshot = Some(snapshot);
        }
        Ok(None) => {
            result.kept_reason = Some("target is already absent".to_owned());
            return result;
        }
        Err(reason) => {
            result.measurement = TargetSize::Unavailable {
                reason: reason.clone(),
            };
            result.kept_reason = Some(reason);
            return result;
        }
    }
    let eligibility = (|| {
        if stored.checkout.occupancy.is_some() {
            return Err("checkout is occupied".to_owned());
        }
        if let Some(reason) = evidence.busy {
            return Err(reason);
        }
        let output = git
            .output(
                &stored.checkout.path,
                &[
                    "ls-files".as_ref(),
                    "--cached".as_ref(),
                    "--others".as_ref(),
                    "--exclude-standard".as_ref(),
                    "-z".as_ref(),
                    "--".as_ref(),
                    "target".as_ref(),
                ],
            )
            .map_err(|error| error.to_string())?;
        if !output.status.success() || !output.stdout.is_empty() {
            return Err(
                "target contains tracked or nonignored user files, or Git could not verify it"
                    .to_owned(),
            );
        }
        if let Some(ended) = evidence.ended_sessions {
            return Ok(format!("observed ended sessions: {ended}"));
        }
        let base = result
            .policy
            .base_ref
            .as_deref()
            .ok_or("no observed ended session or configured LOCAL ancestry proof")?;
        local_ancestry(git, &stored, &main, base)
    })();
    match eligibility {
        Ok(proof) => {
            result.eligibility = Some(proof);
            let size = result
                .snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.bytes);
            result.confirmation_required = !(result.policy.automatic
                && result
                    .policy
                    .projects
                    .iter()
                    .any(|name| name == result.checkout.project.as_str())
                && size <= result.policy.confirm_above_bytes);
        }
        Err(reason) => result.kept_reason = Some(reason),
    }
    result
}

pub(super) fn local_ancestry(
    git: &Git,
    clone: &StoredCheckout,
    main: &StoredCheckout,
    base: &str,
) -> Result<String, String> {
    if !base.starts_with("refs/") || base.contains(['\n', '\r', '\0']) {
        return Err("LOCAL base must be an explicit full ref".to_owned());
    }
    let main_root = crate::storage::open_root(&main.checkout.path)?;
    crate::storage::verify_marker(main, &main_root)?;
    let root_identity = mount::identity(&main_root)?.0;
    let run = |dir: &std::path::Path, args: &[&str]| -> Result<String, String> {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        git.run(dir, &args)
            .map(|s| s.trim().to_owned())
            .map_err(|e| e.to_string())
    };
    run(&main.checkout.path, &["check-ref-format", base])?;
    let branch = run(&clone.checkout.path, &["symbolic-ref", "--quiet", "HEAD"])?;
    let tip = run(
        &clone.checkout.path,
        &["rev-parse", "--verify", "HEAD^{commit}"],
    )?;
    let base_oid = run(
        &main.checkout.path,
        &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
    )?;
    run(
        &main.checkout.path,
        &["merge-base", "--is-ancestor", &tip, &base_oid],
    )?;
    if root_identity != mount::identity(&crate::storage::open_root(&main.checkout.path)?)?.0 {
        return Err("LOCAL base checkout changed".to_owned());
    }
    Ok(format!(
        "LOCAL ancestry: {branch} {tip} is ancestor of {base} {base_oid}"
    ))
}

pub(super) fn observe(stored: &StoredCheckout, name: &[u8]) -> Result<Option<Snapshot>, String> {
    let root = crate::storage::open_root(&stored.checkout.path)?;
    crate::storage::verify_marker(stored, &root)?;
    let root_identity = mount::identity(&root)?.0;
    let mount_id = mount::mount_id(&root)?;
    let name = CString::new(name).map_err(|e| e.to_string())?;
    if matches!(
        fs::statat(
            &root,
            &name,
            AtFlags::SYMLINK_NOFOLLOW | AtFlags::NO_AUTOMOUNT
        ),
        Err(rustix::io::Errno::NOENT)
    ) {
        return Ok(None);
    }
    let target = mount::open_child(&root, &name, true, mount_id)?;
    let target_identity = mount::identity(&target)?.0;
    let snapshot = snapshot_directory(
        target,
        root_identity.clone(),
        target_identity.clone(),
        mount_id,
    )?;
    let again = mount::open_child(&root, &name, true, mount_id)?;
    if mount::identity(&again)?.0 != target_identity
        || mount::identity(&crate::storage::open_root(&stored.checkout.path)?)?.0 != root_identity
    {
        return Err("checkout or target was replaced during observation".to_owned());
    }
    crate::storage::verify_marker(stored, &root)?;
    Ok(Some(snapshot))
}

pub(super) fn snapshot_directory(
    target: OwnedFd,
    root: String,
    target_identity: String,
    mount_id: u64,
) -> Result<Snapshot, String> {
    let mut scan = Scan {
        entries: Vec::new(),
        bytes: 0,
        encoded: 0,
        visited: 0,
        mount: mount_id,
        deadline: Instant::now() + Duration::from_secs(5),
    };
    scan.directory(target, Vec::new(), 0)?;
    Ok(Snapshot {
        root,
        target: target_identity,
        entries: scan.entries,
        bytes: scan.bytes,
    })
}

struct Scan {
    entries: Vec<Entry>,
    bytes: u64,
    encoded: usize,
    visited: usize,
    mount: u64,
    deadline: Instant,
}
impl Scan {
    fn entry(&mut self, path: Vec<u8>, fd: &OwnedFd) -> Result<(), String> {
        let (identity, bytes, directory) = mount::identity(fd)?;
        self.encoded = self
            .encoded
            .saturating_add(path.len())
            .saturating_add(identity.len())
            .saturating_add(17);
        if self.encoded > manifest::MAX_BYTES || self.entries.len() >= manifest::MAX_ENTRIES {
            return Err("target manifest limit reached; measurement incomplete".to_owned());
        }
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or("target byte count overflow")?;
        self.entries.push(Entry {
            path,
            identity,
            bytes,
            directory,
        });
        Ok(())
    }
    fn directory(&mut self, fd: OwnedFd, path: Vec<u8>, depth: usize) -> Result<(), String> {
        if depth > 64 {
            return Err("target depth bound reached; measurement incomplete".to_owned());
        }
        self.entry(path.clone(), &fd)?;
        let mut dir = Dir::new(fd).map_err(|e| e.to_string())?;
        while let Some(entry) = dir.read() {
            self.visited += 1;
            if self.visited > manifest::MAX_ENTRIES || Instant::now() >= self.deadline {
                return Err("target scan bound reached; measurement incomplete".to_owned());
            }
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            if matches!(name.to_bytes(), b"." | b"..") {
                continue;
            }
            if matches!(
                name.to_bytes(),
                b".git" | b".torlek" | b".cockpit" | b".codex" | b".claude"
            ) {
                return Err("target contains repository or application state".to_owned());
            }
            let parent = dir.fd().map_err(|e| e.to_string())?;
            let pinned = mount::open_child(parent, name, false, self.mount)?;
            let (identity, _, is_dir) = mount::identity(&pinned)?;
            let mut relative = path.clone();
            if !relative.is_empty() {
                relative.push(b'/');
            }
            relative.extend_from_slice(name.to_bytes());
            if is_dir {
                let child = mount::open_child(parent, name, true, self.mount)?;
                if mount::identity(&child)?.0 != identity {
                    return Err("target directory changed before traversal".to_owned());
                }
                self.directory(child, relative, depth + 1)?;
            } else {
                self.entry(relative, &pinned)?;
            }
        }
        // Stable ordering makes filesystem enumeration order irrelevant to plan equality.
        if depth == 0 {
            self.entries.sort_by(|a, b| a.path.cmp(&b.path));
        }
        Ok(())
    }
}
