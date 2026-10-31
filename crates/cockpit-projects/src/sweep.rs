//! The clone sweep: deletes clones nobody occupies and that hold no work, keeps the rest
//! with the reason.

use std::fs;
use std::path::Path;

use cockpit_turso::Database;

use crate::git::arg;
use crate::store::{self, GIT_DIR_MARKER_PREFIX, StoredCheckout};
use crate::{Checkout, CheckoutId, CheckoutState, Error, Git, Holder, blocking, clones_dir};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SweepReport {
    /// Deleted, or that a dry run would delete.
    pub removed: Vec<Checkout>,
    pub kept: Vec<(Checkout, Kept)>,
}

/// Why the sweep kept a clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kept {
    Occupied {
        holder: Holder,
    },
    /// `git status` lists changes not committed, or a stash holds some.
    UncommittedChanges,
    /// A local branch or HEAD has commits no remote-tracking branch has.
    UnpushedCommits,
    /// The working copy is not the recorded incarnation; the owner decides what it is.
    Replaced {
        reason: String,
    },
    /// Git could not answer, or deleting failed; nothing is deleted on doubt.
    Unreadable {
        reason: String,
    },
}

/// Deletes every parallel and review clone that is unoccupied, still the recorded
/// incarnation, has no uncommitted change, no stash and no commit missing from its remotes.
/// Age never counts, and ignored files do not either: they are mostly build output. With
/// `dry_run`, deletes nothing and reports what it would delete. A clone nobody holds whose
/// directory is already gone is recorded removed, and a clone with submodules is kept.
pub async fn sweep(database: &Database, git: &Git, dry_run: bool) -> Result<SweepReport, Error> {
    let clones = {
        let read = database.read().await?;
        store::stored_clones(&read).await?
    };
    let mut report = SweepReport::default();
    for stored in clones {
        let kept = if dry_run {
            inspect(git, &stored).await?
        } else {
            sweep_one(database, git, &stored).await?
        };
        match kept {
            None => report.removed.push(stored.checkout),
            Some(kept) => report.kept.push((stored.checkout, kept)),
        }
    }
    Ok(report)
}

/// What the sweep knows about a clone before asking Git.
enum Precheck {
    Keep(Kept),
    /// Its directory no longer exists: nothing in it can be lost.
    Gone,
    Present,
}

/// [`precheck_now`] off the async workers: it reads the disk.
async fn precheck(stored: &StoredCheckout) -> Result<Precheck, Error> {
    let stored = stored.clone();
    blocking(move || Ok(precheck_now(&stored))).await
}

/// A holder keeps the clone even when its directory is gone: the holder decides.
fn precheck_now(stored: &StoredCheckout) -> Precheck {
    if let Some(occupancy) = &stored.checkout.occupancy {
        return Precheck::Keep(Kept::Occupied {
            holder: occupancy.holder.clone(),
        });
    }
    let is_gone = fs::symlink_metadata(&stored.checkout.path)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
    if is_gone {
        return Precheck::Gone;
    }
    // A working copy that is not the recorded incarnation, a half-deleted clone replaced
    // before the next sweep included.
    match stored.verify_on_disk_now() {
        Ok(()) => Precheck::Present,
        Err(error) => Precheck::Keep(Kept::Replaced {
            reason: error.to_string(),
        }),
    }
}

/// Why a dry run keeps the clone, or `None` when the sweep would delete it.
async fn inspect(git: &Git, stored: &StoredCheckout) -> Result<Option<Kept>, Error> {
    match precheck(stored).await? {
        Precheck::Keep(kept) => Ok(Some(kept)),
        Precheck::Gone => Ok(None),
        Precheck::Present if stored.checkout.state == CheckoutState::Deleting => Ok(None),
        Precheck::Present => leftovers(git, &stored.checkout.path).await,
    }
}

/// Deletes the clone unless it must be kept; `Some` says why it was.
async fn sweep_one(
    database: &Database,
    git: &Git,
    stored: &StoredCheckout,
) -> Result<Option<Kept>, Error> {
    let checkout = &stored.checkout;
    let is_gone = match precheck(stored).await? {
        Precheck::Keep(kept) => return Ok(Some(kept)),
        Precheck::Gone => true,
        Precheck::Present => false,
    };
    if checkout.state == CheckoutState::Live
        && let Some(kept) = mark_removing(database, checkout.id).await?
    {
        return Ok(Some(kept));
    }
    if checkout.state != CheckoutState::Deleting {
        // Looked at once claims are refused, so work done before the mark is seen, and again
        // after an interrupted or concurrent check: only `deleting` means the checks passed.
        if !is_gone && let Some(kept) = leftovers(git, &checkout.path).await? {
            let write = database.write().await?;
            store::mark_live(&write, checkout.id).await?;
            write.commit().await?;
            return Ok(Some(kept));
        }
        let write = database.write().await?;
        let is_marked = store::mark_deleting(&write, checkout.id).await?;
        write.commit().await?;
        if !is_marked {
            return Ok(Some(Kept::Unreadable {
                reason: "its state changed during the sweep".to_owned(),
            }));
        }
    }
    let expected_parent = clones_dir(&checkout.project_root, &checkout.project)?;
    let path = checkout.path.clone();
    let deleted = blocking(move || delete_clone_dir(&path, &expected_parent)).await;
    if let Err(error) = deleted {
        return Ok(Some(Kept::Unreadable {
            reason: error.to_string(),
        }));
    }
    let write = database.write().await?;
    store::mark_removed(&write, checkout.id).await?;
    write.commit().await?;
    Ok(None)
}

/// Marks the clone as being removed, which refuses claims from then on. `Some` when someone
/// claimed it first.
async fn mark_removing(database: &Database, id: CheckoutId) -> Result<Option<Kept>, Error> {
    let write = database.write().await?;
    let is_marked = store::mark_removing(&write, id).await?;
    write.commit().await?;
    if is_marked {
        return Ok(None);
    }
    let read = database.read().await?;
    let holder = store::stored_checkout(&read, id)
        .await?
        .checkout
        .occupancy
        .map(|occupancy| occupancy.holder);
    Ok(Some(match holder {
        Some(holder) => Kept::Occupied { holder },
        None => Kept::Unreadable {
            reason: "its state changed during the sweep".to_owned(),
        },
    }))
}

/// Work the clone holds that exists nowhere else, or why Git could not tell.
async fn leftovers(git: &Git, clone: &Path) -> Result<Option<Kept>, Error> {
    let git = git.clone();
    let clone = clone.to_path_buf();
    blocking(move || Ok(leftovers_now(&git, &clone))).await
}

fn leftovers_now(git: &Git, clone: &Path) -> Option<Kept> {
    let unreadable = |error: Error| {
        Some(Kept::Unreadable {
            reason: error.to_string(),
        })
    };
    // `git status` would run each submodule's own Git, under configuration the driver reset
    // never read; nothing is deleted on doubt.
    if clone.join(".gitmodules").exists() || clone.join(".git/modules").exists() {
        return Some(Kept::Unreadable {
            reason: "it has submodules, which the sweep does not inspect".to_owned(),
        });
    }
    // A committed repository inside the clone (a gitlink) without `.gitmodules`: status
    // would not look inside it.
    match git.run(clone, &[arg("ls-files"), arg("--stage")]) {
        Ok(index) if index.lines().any(|line| line.starts_with("160000 ")) => {
            return Some(Kept::Unreadable {
                reason: "it holds a nested repository, which the sweep does not inspect".to_owned(),
            });
        }
        Ok(_) => {}
        Err(error) => return unreadable(error),
    }
    let status = git.run(
        clone,
        &[
            arg("status"),
            arg("--porcelain"),
            arg("--ignore-submodules=all"),
        ],
    );
    match status {
        Ok(status) if !status.trim().is_empty() => return Some(Kept::UncommittedChanges),
        Ok(_) => {}
        Err(error) => return unreadable(error),
    }
    let stash = git.output(
        clone,
        &[
            arg("rev-parse"),
            arg("--verify"),
            arg("--quiet"),
            arg("refs/stash"),
        ],
    );
    match stash {
        Ok(stash) if stash.status.success() => return Some(Kept::UncommittedChanges),
        Ok(_) => {}
        Err(error) => return unreadable(error),
    }
    // HEAD too: commits made on a detached HEAD are on no branch.
    let unpushed = git.run(
        clone,
        &[
            arg("log"),
            arg("-1"),
            arg("--format=%H"),
            arg("HEAD"),
            arg("--branches"),
            arg("--not"),
            arg("--remotes"),
        ],
    );
    match unpushed {
        Ok(commits) if !commits.trim().is_empty() => Some(Kept::UnpushedCommits),
        Ok(_) => None,
        Err(error) => unreadable(error),
    }
}

/// Deletes `clone` only when it is a real directory directly inside its project's clones
/// directory; a symbolic link or a path elsewhere is refused. An already missing directory is
/// a finished deletion. The project's clones directory goes too once empty.
fn delete_clone_dir(clone: &Path, expected_parent: &Path) -> Result<(), Error> {
    let refuse = |reason: &str| Error::Refused {
        reason: format!("{}: {reason}", clone.display()),
    };
    if clone.parent() != Some(expected_parent) {
        return Err(refuse(
            "is not directly inside the project's clones directory",
        ));
    }
    let metadata = match fs::symlink_metadata(clone) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Error::io("inspect", clone)(error)),
    };
    if !metadata.is_dir() {
        return Err(refuse("is not a directory (a symbolic link is refused)"));
    }
    let resolved = clone.canonicalize().map_err(Error::io("resolve", clone))?;
    if resolved != clone {
        return Err(refuse("resolves elsewhere through a symbolic link"));
    }
    // The marker last: an interrupted deletion keeps the proof that what is left is still
    // this clone, so the next sweep finishes it.
    remove_entries(clone, |name| name == ".git")?;
    let git_dir = clone.join(".git");
    remove_entries(&git_dir, |name| {
        name.to_string_lossy().starts_with(GIT_DIR_MARKER_PREFIX)
    })?;
    remove_entries(&git_dir, |_| false)?;
    fs::remove_dir(&git_dir).map_err(Error::io("delete", &git_dir))?;
    fs::remove_dir(clone).map_err(Error::io("delete", clone))?;
    // Fails while another clone is there, which is the point.
    let _ = fs::remove_dir(expected_parent);
    Ok(())
}

/// Deletes every entry of `dir` except those `keep` names.
fn remove_entries(dir: &Path, keep: impl Fn(&std::ffi::OsStr) -> bool) -> Result<(), Error> {
    for entry in fs::read_dir(dir).map_err(Error::io("list", dir))? {
        let entry = entry.map_err(Error::io("list", dir))?;
        if keep(&entry.file_name()) {
            continue;
        }
        let path = entry.path();
        let is_dir = entry
            .file_type()
            .map_err(Error::io("inspect", &path))?
            .is_dir();
        let removed = if is_dir {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        removed.map_err(Error::io("delete", &path))?;
    }
    Ok(())
}
