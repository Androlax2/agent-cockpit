//! The clone sweep: deletes clones nobody occupies and that hold no work, keeps the rest
//! with the reason.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::time::Duration;

use cockpit_turso::Database;

use crate::git::{arg, git_failure, is_github_url};
use crate::push::configured_redirects;
use crate::store::{self, GIT_DIR_MARKER_PREFIX, StoredCheckout};
use crate::{
    Checkout, CheckoutId, CheckoutKind, CheckoutState, Error, Git, Holder, blocking, clones_dir,
};

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
    /// A local branch or HEAD has commits no remote-tracking branch has, and their changes are
    /// not in `origin/HEAD` either (not squash-merged).
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

impl std::fmt::Display for Kept {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Occupied { holder } => write!(formatter, "occupied by {holder}"),
            Self::UncommittedChanges => formatter.write_str("uncommitted changes"),
            Self::UnpushedCommits => formatter.write_str(
                "unpushed commits (on no remote branch, and not merged into origin/HEAD)",
            ),
            Self::Replaced { reason } => write!(formatter, "replaced: {reason}"),
            Self::Unreadable { reason } => write!(formatter, "unreadable: {reason}"),
        }
    }
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
        let kept = sweep_stored(database, git, &stored, dry_run).await?;
        match kept {
            None => report.removed.push(stored.checkout),
            Some(kept) => report.kept.push((stored.checkout, kept)),
        }
    }
    Ok(report)
}

/// [`sweep`] of the one clone `checkout`, as a session's end asks for it (#767): the clone as
/// it was before the sweep and why it was kept, `None` when it was deleted. A main checkout is
/// refused.
pub async fn sweep_clone(
    database: &Database,
    git: &Git,
    checkout: CheckoutId,
) -> Result<(Checkout, Option<Kept>), Error> {
    let stored = {
        let read = database.read().await?;
        store::stored_checkout(&read, checkout).await?
    };
    if stored.checkout.kind == CheckoutKind::Main {
        return Err(Error::Refused {
            reason: format!("checkout {checkout} is a project's main checkout, not a clone"),
        });
    }
    let kept = sweep_stored(database, git, &stored, false).await?;
    Ok((stored.checkout, kept))
}

/// Deletes one clone, or with `dry_run` says whether it would; `Some` says why it was kept.
async fn sweep_stored(
    database: &Database,
    git: &Git,
    stored: &StoredCheckout,
    dry_run: bool,
) -> Result<Option<Kept>, Error> {
    let available = {
        let read = database.read().await?;
        crate::require_checkout_available(&read, stored.checkout.id).await
    };
    match available {
        Err(Error::Refused { reason }) => return Ok(Some(Kept::Unreadable { reason })),
        Err(error) => return Err(error),
        Ok(()) => {}
    }
    if dry_run {
        inspect(git, stored).await
    } else {
        sweep_one(database, git, stored).await
    }
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
    let git = git.clone();
    let stored = stored.clone();
    blocking(move || {
        Ok(match precheck_now(&stored) {
            Precheck::Keep(kept) => Some(kept),
            Precheck::Gone => None,
            Precheck::Present if stored.checkout.state == CheckoutState::Deleting => None,
            Precheck::Present => {
                let origin = ask_origin(&git, &stored.checkout);
                leftovers_now(&git, &stored.checkout.path, &origin)
            }
        })
    })
    .await
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
    // Before the mark, which refuses claims: origin can take seconds to answer.
    let origin = if is_gone || checkout.state == CheckoutState::Deleting {
        Origin::NotNeeded
    } else {
        let git = git.clone();
        let checkout = checkout.clone();
        blocking(move || Ok(ask_origin(&git, &checkout))).await?
    };
    if checkout.state == CheckoutState::Live
        && let Some(kept) = mark_removing(database, checkout.id).await?
    {
        return Ok(Some(kept));
    }
    if checkout.state != CheckoutState::Deleting {
        // Looked at once claims are refused, so work done before the mark is seen, and again
        // after an interrupted or concurrent check: only `deleting` means the checks passed.
        if !is_gone && let Some(kept) = leftovers(git, &checkout.path, origin).await? {
            let write = database.write().await?;
            store::mark_live(&write, checkout.id).await?;
            write.commit().await?;
            return Ok(Some(kept));
        }
        let write = database.write().await?;
        let is_marked = store::mark_deleting(&write, checkout.id).await?;
        write.commit().await?;
        if !is_marked {
            return Ok(Some(state_changed()));
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
        None => state_changed(),
    }))
}

/// A row whose state another sweep or a claim changed since this sweep read it.
fn state_changed() -> Kept {
    Kept::Unreadable {
        reason: "its state changed during the sweep".to_owned(),
    }
}

/// Work the clone holds that exists nowhere else, or why Git could not tell.
async fn leftovers(git: &Git, clone: &Path, origin: Origin) -> Result<Option<Kept>, Error> {
    let git = git.clone();
    let clone = clone.to_path_buf();
    blocking(move || Ok(leftovers_now(&git, &clone, &origin))).await
}

/// `origin` is what origin said before; a commit made since is on none of its branches.
fn leftovers_now(git: &Git, clone: &Path, origin: &Origin) -> Option<Kept> {
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
    let remote_heads: &[String] = match origin {
        Origin::Heads(heads) => heads,
        Origin::NotNeeded | Origin::NotAsked | Origin::Unanswered(_) => &[],
    };
    match has_unpushed_commits(git, clone, remote_heads) {
        Ok(false) => None,
        Ok(true) => Some(match origin {
            Origin::Unanswered(error) => Kept::Unreadable {
                reason: format!(
                    "its remote-tracking refs lack some of its commits, and origin could not be \
                     asked whether it has them: {error}"
                ),
            },
            Origin::NotNeeded | Origin::NotAsked | Origin::Heads(_) => Kept::UnpushedCommits,
        }),
        Err(error) => unreadable(error),
    }
}

/// What origin said about its branches (#882): the clone's remote-tracking refs can be stale,
/// so origin itself has the last word.
enum Origin {
    /// Every commit of the clone is on a remote-tracking branch: there was nothing to ask.
    NotNeeded,
    /// It is not an origin the sweep asks; see [`remote_branch_heads`].
    NotAsked,
    /// The commits its branches point at, those the clone has.
    Heads(Vec<String>),
    /// Why it could not be asked, or did not answer.
    Unanswered(String),
}

/// Asks origin for its branches when HEAD or a local branch has a commit no remote-tracking
/// branch has.
fn ask_origin(git: &Git, checkout: &Checkout) -> Origin {
    let clone = checkout.path.as_path();
    let asked = has_commit_off(git, clone, &[]).and_then(|is_off| {
        if !is_off {
            return Ok(None);
        }
        remote_branch_heads(git, checkout).map(Some)
    });
    match asked {
        Ok(None) => Origin::NotNeeded,
        Ok(Some(None)) => Origin::NotAsked,
        Ok(Some(Some(heads))) => Origin::Heads(heads),
        Err(error) => Origin::Unanswered(error.to_string()),
    }
}

/// How long the sweep waits for origin to list its branches.
const REMOTE_ANSWER_LIMIT: Duration = Duration::from_secs(20);

/// The commits origin's branches point at now that the clone has, asked of origin itself: a push by another URL,
/// or a rebase pushed from elsewhere, leaves the clone's remote-tracking refs behind (#882).
/// `None`, origin not asked, unless it is one GitHub repository URL or the project's main
/// checkout (a project without a remote), and the clone's own configuration redirects nothing:
/// that configuration is written by whoever works in the clone. Git only reads; no object or
/// ref is written.
fn remote_branch_heads(git: &Git, checkout: &Checkout) -> Result<Option<Vec<String>>, Error> {
    let clone = checkout.path.as_path();
    // The URLs as Git will use them, `insteadOf` applied.
    let urls = git.output(
        clone,
        &[arg("remote"), arg("get-url"), arg("--all"), arg("origin")],
    )?;
    if !urls.status.success() {
        return Ok(None);
    }
    let urls = String::from_utf8_lossy(&urls.stdout).into_owned();
    let [url] = urls.lines().collect::<Vec<_>>()[..] else {
        return Ok(None);
    };
    let extra_config: &[&str] = if is_github_url(url) {
        &["core.sshCommand=ssh -o BatchMode=yes"]
    } else if Path::new(url) == checkout.project_root {
        &["protocol.file.allow=always"]
    } else {
        return Ok(None);
    };
    if configured_redirects(git, clone)?.is_some() {
        return Ok(None);
    }
    let arguments = [arg("ls-remote"), arg("--heads"), arg("origin")];
    let listed = git.output_within(clone, extra_config, &arguments, REMOTE_ANSWER_LIMIT)?;
    if !listed.status.success() {
        return Err(git_failure(&arguments, &listed));
    }
    let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
    let heads: Vec<&str> = listed
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(commit, _)| commit)
        .filter(|commit| commit.len() >= 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .collect();
    if heads.is_empty() {
        return Ok(Some(Vec::new()));
    }
    // Only the heads the clone has: a missing one would fail every later query, and
    // `--ignore-missing` there would hide a missing HEAD or branch as well.
    let mut arguments = vec![arg("rev-list"), arg("--no-walk"), arg("--ignore-missing")];
    arguments.extend(heads.iter().map(arg));
    let present = git.run(clone, &arguments)?;
    Ok(Some(present.lines().map(str::to_owned).collect()))
}

/// `--not` with the remote-tracking branches and `remote_heads`, the branch heads origin
/// reported that the clone has.
fn not_on_a_remote(remote_heads: &[String]) -> impl Iterator<Item = &OsStr> {
    [arg("--not"), arg("--remotes")]
        .into_iter()
        .chain(remote_heads.iter().map(arg))
}

/// Whether HEAD or a local branch has a commit that neither a remote-tracking branch nor one of
/// `remote_heads` (commits origin reported its branches at) has, unless the changes of each
/// such tip are already in the remote's default branch. HEAD too: commits made on a detached
/// HEAD are on no branch.
pub(crate) fn has_unpushed_commits(
    git: &Git,
    clone: &Path,
    remote_heads: &[String],
) -> Result<bool, Error> {
    if !has_commit_off(git, clone, remote_heads)? {
        return Ok(false);
    }
    Ok(!is_merged_into_default_branch(git, clone, remote_heads)?)
}

/// Whether HEAD or a local branch has a commit that neither a remote-tracking branch nor one of
/// `remote_heads` has, merged or not.
fn has_commit_off(git: &Git, clone: &Path, remote_heads: &[String]) -> Result<bool, Error> {
    let mut arguments = vec![
        arg("log"),
        arg("-1"),
        arg("--format=%H"),
        arg("HEAD"),
        arg("--branches"),
    ];
    arguments.extend(not_on_a_remote(remote_heads));
    Ok(!git.run(clone, &arguments)?.trim().is_empty())
}

/// Whether the content of every local branch and HEAD holding a commit no remote-tracking branch
/// has is already in `origin/HEAD`'s history: each path such a commit touched reads at the tip
/// as it does in `origin/HEAD`, or as a commit of `origin/HEAD` since their merge base left it.
/// That is a pull request squash-merged (or rebased) after which its remote branch was deleted
/// and pruned by a fetch (#767), the default branch possibly changing the same files since;
/// only the commits' messages and their intermediate states are lost. The answer is `false`,
/// kept on doubt, without `origin/HEAD`, without a merge base, when a tip changes nothing since
/// it (an empty commit, or one undoing the whole change), and when a touched path reads at the
/// tip as no commit of `origin/HEAD` left it (a later commit undoing part of the merged
/// change). Git only reads: no object is written, so `torlek storage` stays read-only.
fn is_merged_into_default_branch(
    git: &Git,
    clone: &Path,
    remote_heads: &[String],
) -> Result<bool, Error> {
    let default = git.output(
        clone,
        &[
            arg("rev-parse"),
            arg("--verify"),
            arg("--quiet"),
            arg("refs/remotes/origin/HEAD^{commit}"),
        ],
    )?;
    if !default.status.success() {
        return Ok(false);
    }
    let default = String::from_utf8_lossy(&default.stdout).trim().to_owned();
    let branch_tips = git.run(
        clone,
        &[
            arg("for-each-ref"),
            arg("--format=%(objectname)"),
            arg("refs/heads"),
        ],
    )?;
    let head = git.run(clone, &[arg("rev-parse"), arg("--verify"), arg("HEAD")])?;
    let mut tips: Vec<&str> = branch_tips.lines().chain(head.lines()).collect();
    tips.sort_unstable();
    tips.dedup();
    for tip in tips {
        let mut arguments = vec![arg("rev-list"), arg("-1"), arg(tip)];
        arguments.extend(not_on_a_remote(remote_heads));
        let unpushed = git.run(clone, &arguments)?;
        if !unpushed.trim().is_empty() && !is_tip_merged(git, clone, &default, tip, remote_heads)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Whether every path `tip`'s unpushed commits touched reads at `tip` as `default` or one of
/// its commits since their merge base left it; see [`is_merged_into_default_branch`].
fn is_tip_merged(
    git: &Git,
    clone: &Path,
    default: &str,
    tip: &str,
    remote_heads: &[String],
) -> Result<bool, Error> {
    let arguments = [arg("merge-base"), arg(default), arg(tip)];
    let base = git.output(clone, &arguments)?;
    match base.status.code() {
        Some(0) => {}
        // No common history: nothing to compare, so not a merge either.
        Some(1) => return Ok(false),
        _ => return Err(git_failure(&arguments, &base)),
    }
    let base = String::from_utf8_lossy(&base.stdout).trim().to_owned();
    let arguments = [
        arg("diff"),
        arg("--quiet"),
        arg("--no-ext-diff"),
        arg(&base),
        arg(tip),
    ];
    let changes = git.output(clone, &arguments)?;
    match changes.status.code() {
        Some(1) => {}
        Some(0) => return Ok(false),
        _ => return Err(git_failure(&arguments, &changes)),
    }
    // A merge commit counts with what it changed from its first parent, conflict resolutions
    // included.
    let mut arguments = vec![
        arg("log"),
        arg("--format="),
        arg("--name-only"),
        arg("-z"),
        arg("--no-renames"),
        arg("--diff-merges=first-parent"),
        arg(tip),
    ];
    arguments.extend(not_on_a_remote(remote_heads));
    let touched = git.run(clone, &arguments)?;
    let differing = git.run(
        clone,
        &[
            arg("diff"),
            arg("--raw"),
            arg("-z"),
            arg("--no-abbrev"),
            arg("--no-renames"),
            arg(default),
            arg(tip),
        ],
    )?;
    let at_tip: HashMap<&str, &str> = raw_entries(&differing).collect();
    let mut touched_differing = touched
        .split('\0')
        .filter(|path| at_tip.contains_key(path))
        .peekable();
    if touched_differing.peek().is_none() {
        return Ok(true);
    }
    let landed = git.run(
        clone,
        &[
            arg("log"),
            arg("--format="),
            arg("--raw"),
            arg("-z"),
            arg("--no-abbrev"),
            arg("--no-renames"),
            arg("--diff-merges=first-parent"),
            arg(&format!("{base}..{default}")),
        ],
    )?;
    let landed: HashSet<(&str, &str)> = raw_entries(&landed).collect();
    Ok(touched_differing.all(|path| landed.contains(&(path, at_tip[path]))))
}

/// The `(path, object after)` of each entry of Git's `--raw -z` output, `:mode mode object
/// object status` then the path; a deleted path's object after is all zeros.
fn raw_entries(output: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut fields = output.split('\0');
    std::iter::from_fn(move || {
        let meta = fields.next()?.trim_start_matches('\n');
        let path = fields.next()?;
        let after = meta.split(' ').nth(3)?;
        Some((path, after))
    })
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
