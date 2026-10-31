//! Parallel and review clones: fast local clones of the main checkout that share nothing
//! with it afterwards.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cockpit_turso::Database;

use crate::git::arg;
use crate::store::{self, GitDirIdentity};
use crate::{Checkout, CheckoutKind, Error, Git, Project, blocking, clones_dir};

/// Git hooks lefthook can manage; a clone gets Cockpit's runner for those its config names.
const LEFTHOOK_GIT_HOOKS: &[&str] = &[
    "applypatch-msg",
    "pre-applypatch",
    "post-applypatch",
    "pre-commit",
    "pre-merge-commit",
    "prepare-commit-msg",
    "commit-msg",
    "post-commit",
    "pre-rebase",
    "post-checkout",
    "post-merge",
    "pre-push",
    "pre-auto-gc",
    "post-rewrite",
    "reference-transaction",
    "push-to-checkout",
    "sendemail-validate",
    "fsmonitor-watchman",
    "post-index-change",
];
const LEFTHOOK_CONFIGS: &[&str] = &[
    "lefthook.yml",
    ".lefthook.yml",
    "lefthook.yaml",
    ".lefthook.yaml",
];

/// Clones `project` for work on `branch` into `<clones dir>/<branch with / as ->`, switches to
/// the branch (created when it does not exist), installs Cockpit's lefthook runners and
/// records the clone. It inherits the project's trust ([`crate::trust`]).
pub async fn create_parallel_clone(
    database: &Database,
    git: &Git,
    project: &str,
    branch: &str,
) -> Result<Checkout, Error> {
    let project = source(database, project).await?;
    let path = clones_dir(&project.root, &project.name)?.join(branch.replace('/', "-"));
    let kind = CheckoutKind::Parallel {
        branch: branch.to_owned(),
    };
    let git = git.clone();
    let branch = branch.to_owned();
    let main_root = project.root.clone();
    let created = path.clone();
    let identity = blocking(move || {
        git.run(
            &main_root,
            &[arg("check-ref-format"), arg("--branch"), arg(&branch)],
        )
        .map_err(|_| Error::InvalidBranch {
            branch: branch.clone(),
        })?;
        build_clone(&git, &main_root, &created, |clone| {
            let switch = git.run(clone, &[arg("switch"), arg("--quiet"), arg(&branch)]);
            if switch.is_err() {
                git.run(
                    clone,
                    &[arg("switch"), arg("--quiet"), arg("-c"), arg(&branch)],
                )?;
            }
            install_lefthook_runners(clone)
        })
    })
    .await?;
    record(database, &project, &kind, &path, &identity).await
}

/// Clones `project` and checks out the head of its GitHub pull request `pull_request`,
/// detached, into `<clones dir>/pr-<number>` (with the time appended when that exists).
/// No hooks are installed and the clone is never trusted: it holds someone else's changes.
pub async fn create_review_clone(
    database: &Database,
    git: &Git,
    project: &str,
    pull_request: u64,
) -> Result<Checkout, Error> {
    store::pull_request_number(pull_request)?;
    let project = source(database, project).await?;
    let main_root = project.root.clone();
    let clone_dir = clones_dir(&project.root, &project.name)?;
    let mut path = clone_dir.join(format!("pr-{pull_request}"));
    if fs::symlink_metadata(&path).is_ok() {
        path = clone_dir.join(format!("pr-{pull_request}-{}", store::now()));
    }
    let git = git.clone();
    let created = path.clone();
    let identity = blocking(move || {
        build_clone(&git, &main_root, &created, |clone| {
            // A remote-tracking ref, not FETCH_HEAD, so the sweep counts the pull request's
            // commits as pushed and only commits made in the clone as work.
            let tracking = format!("refs/remotes/origin/pull/{pull_request}");
            let refspec = format!("pull/{pull_request}/head:{tracking}");
            git.run(
                clone,
                &[arg("fetch"), arg("--quiet"), arg("origin"), arg(&refspec)],
            )?;
            git.run(
                clone,
                &[
                    arg("switch"),
                    arg("--quiet"),
                    arg("--detach"),
                    arg(&tracking),
                ],
            )?;
            Ok(())
        })
    })
    .await?;
    let kind = CheckoutKind::Review { pull_request };
    record(database, &project, &kind, &path, &identity).await
}

/// The project, after checking its main checkout, at the project root, is still the
/// recorded incarnation.
async fn source(database: &Database, project: &str) -> Result<Project, Error> {
    let read = database.read().await?;
    let project = store::project_named(&read, project).await?;
    store::main_checkout(&read, project.id)
        .await?
        .verify_on_disk()?;
    Ok(project)
}

/// Records the clone just built at `path`; when that fails, deletes it, so a failed clone
/// leaves no directory.
async fn record(
    database: &Database,
    project: &Project,
    kind: &CheckoutKind,
    path: &Path,
    identity: &GitDirIdentity,
) -> Result<Checkout, Error> {
    match insert(database, project, kind, path, identity).await {
        Ok(checkout) => Ok(checkout),
        Err(error) => {
            let built = path.to_path_buf();
            let removed = blocking(move || {
                fs::remove_dir_all(&built).map_err(Error::io("remove the unrecorded clone", &built))
            })
            .await;
            Err(error.after_cleanup(removed))
        }
    }
}

async fn insert(
    database: &Database,
    project: &Project,
    kind: &CheckoutKind,
    path: &Path,
    identity: &GitDirIdentity,
) -> Result<Checkout, Error> {
    let write = database.write().await?;
    let checkout = store::insert_checkout(&write, project, kind, path, identity).await?;
    write.commit().await?;
    Ok(checkout)
}

/// Clones `main_root` into `clone`, which must not exist, points its origin at the main
/// checkout's origin and runs `prepare` in it. The directory is created here first, so a
/// concurrent request for the same path fails instead of sharing it, and on failure the
/// partial clone this call owns is deleted.
fn build_clone(
    git: &Git,
    main_root: &Path,
    clone: &Path,
    prepare: impl FnOnce(&Path) -> Result<(), Error>,
) -> Result<GitDirIdentity, Error> {
    let parent = clone
        .parent()
        .expect("a clone path is inside the clones directory");
    fs::create_dir_all(parent).map_err(Error::io("create", parent))?;
    match fs::create_dir(clone) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Error::PathTaken {
                path: clone.to_path_buf(),
            });
        }
        Err(error) => return Err(Error::io("create", clone)(error)),
    }
    clone_and_prepare(git, main_root, clone, prepare).map_err(|error| {
        error.after_cleanup(
            fs::remove_dir_all(clone).map_err(Error::io("remove the partial clone", clone)),
        )
    })
}

fn clone_and_prepare(
    git: &Git,
    main_root: &Path,
    clone: &Path,
    prepare: impl FnOnce(&Path) -> Result<(), Error>,
) -> Result<GitDirIdentity, Error> {
    let parent = clone
        .parent()
        .expect("a clone path is inside the clones directory");
    // Cloning into the empty directory build_clone created. `--no-local` copies objects through the pack protocol instead of hard-linking them,
    // `--reference` lets that copy take them from the main checkout, and `--dissociate`
    // copies what was borrowed, so the clone shares no file with the main checkout (#28).
    // The local source needs the file protocol, which the safe settings refuse otherwise.
    git.run_with(
        parent,
        &["protocol.file.allow=always"],
        &[
            arg("clone"),
            arg("--quiet"),
            arg("--no-local"),
            arg("--reference"),
            main_root.as_os_str(),
            arg("--dissociate"),
            arg("--"),
            main_root.as_os_str(),
            clone.as_os_str(),
        ],
    )?;
    let origin = git.output(main_root, &[arg("remote"), arg("get-url"), arg("origin")])?;
    if origin.status.success() {
        let url = String::from_utf8_lossy(&origin.stdout).trim().to_owned();
        git.run(
            clone,
            &[arg("remote"), arg("set-url"), arg("origin"), arg(&url)],
        )?;
    }
    prepare(clone)?;
    GitDirIdentity::create(clone).map_err(|reason| Error::InvalidPath {
        path: clone.to_path_buf(),
        reason,
    })
}

/// Writes Cockpit's own runner to `.git/hooks/<hook>` for each hook the clone's lefthook
/// config names. The runner's text is Cockpit's, never the repository's: it only calls
/// lefthook, which reads the config the agent works on.
fn install_lefthook_runners(clone: &Path) -> Result<(), Error> {
    let Some(config) = LEFTHOOK_CONFIGS
        .iter()
        .map(|name| clone.join(name))
        .find(|path| {
            fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
        })
    else {
        return Ok(());
    };
    let text = fs::read_to_string(&config).map_err(Error::io("read", &config))?;
    let hooks_dir = clone.join(".git/hooks");
    fs::create_dir_all(&hooks_dir).map_err(Error::io("create", &hooks_dir))?;
    for hook in LEFTHOOK_GIT_HOOKS {
        let is_configured = text.lines().any(|line| {
            line.strip_prefix(hook)
                .is_some_and(|rest| rest.starts_with(':'))
        });
        if !is_configured {
            continue;
        }
        let path = hooks_dir.join(hook);
        fs::write(&path, lefthook_runner(hook)).map_err(Error::io("write", &path))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755))
            .map_err(Error::io("make executable", &path))?;
    }
    Ok(())
}

fn lefthook_runner(hook: &str) -> String {
    format!(
        "#!/bin/sh\n\
         # Installed by Cockpit in a parallel clone: it runs lefthook, nothing else.\n\
         [ \"${{LEFTHOOK:-1}}\" = 0 ] && exit 0\n\
         if [ -n \"$LEFTHOOK_BIN\" ]; then exec \"$LEFTHOOK_BIN\" run {hook} \"$@\"; fi\n\
         if command -v lefthook >/dev/null 2>&1; then exec lefthook run {hook} \"$@\"; fi\n\
         if [ -x node_modules/.bin/lefthook ]; then exec node_modules/.bin/lefthook run {hook} \"$@\"; fi\n\
         echo \"cockpit: lefthook is not installed; the {hook} hook did not run\" >&2\n"
    )
}
