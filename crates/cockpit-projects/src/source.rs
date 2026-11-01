//! The exact source a checkout holds: its head and a digest of its uncommitted changes, so a
//! verification receipt names what was checked (#313, report 09).

use std::ffi::OsStr;
use std::fs::DirBuilder;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::git::{arg, git_failure};
use crate::{Error, Git};

/// A checkout's head and uncommitted changes, as [`Git::source_state`] read them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceState {
    /// The commit checked out.
    pub head: String,
    /// The tree the worktree would commit (staged, unstaged and untracked files, ignored ones
    /// left out), `None` when it is the head's own tree.
    pub dirty: Option<String>,
}

impl Git {
    /// The head of the checkout at `dir` and a digest of its uncommitted changes: the tree
    /// `git add --all` would write, built in a private index and object directory, so the
    /// checkout's own index and objects are left as they were. Symbolic links count as
    /// their target text, never followed; every file is hashed, so it costs one read of the
    /// worktree. Refused for a repository with no commit.
    pub fn source_state(&self, dir: &Path) -> Result<SourceState, Error> {
        let drivers_off = self.drivers_off(dir)?;
        let scratch = Scratch::create()?;
        let run = |environment: &[(&str, &OsStr)], arguments: &[&OsStr]| {
            let mut command = self.command(dir, &[]);
            for setting in &drivers_off {
                command.arg("-c").arg(setting);
            }
            command.args(arguments);
            for (variable, value) in environment {
                command.env(variable, value);
            }
            let output = command.output().map_err(Error::io("run git in", dir))?;
            if !output.status.success() {
                return Err(git_failure(arguments, &output));
            }
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        };
        let head = run(
            &[],
            &[arg("rev-parse"), arg("--verify"), arg("HEAD^{commit}")],
        )?;
        let head_tree = run(
            &[],
            &[arg("rev-parse"), arg("--verify"), arg("HEAD^{tree}")],
        )?;
        let objects = dir.join(run(
            &[],
            &[arg("rev-parse"), arg("--git-path"), arg("objects")],
        )?);
        let index = scratch.path.join("index");
        // Git does not discover a repository whose object directory is missing.
        let new_objects = scratch.path.join("objects");
        std::fs::create_dir(&new_objects).map_err(Error::io("create", &new_objects))?;
        let private = [
            ("GIT_INDEX_FILE", index.as_os_str()),
            ("GIT_OBJECT_DIRECTORY", new_objects.as_os_str()),
            ("GIT_ALTERNATE_OBJECT_DIRECTORIES", objects.as_os_str()),
        ];
        run(&private, &[arg("read-tree"), arg(&head)])?;
        run(&private, &[arg("add"), arg("--all"), arg("--"), arg(".")])?;
        let tree = run(&private, &[arg("write-tree")])?;
        Ok(SourceState {
            head,
            dirty: (tree != head_tree).then_some(tree),
        })
    }
}

/// A private directory under the system's temporary one, removed when dropped. Created
/// `0700` and never reused, so nothing another user placed there is written through.
struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn create() -> Result<Scratch, Error> {
        static CREATED: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cockpit-source-{}-{}",
            std::process::id(),
            CREATED.fetch_add(1, Ordering::Relaxed)
        ));
        DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(Error::io("create", &path))?;
        Ok(Scratch { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        // This crate does not log; the daemon's standard error reaches its journal.
        if let Err(error) = std::fs::remove_dir_all(&self.path) {
            eprintln!("cockpit: could not remove {}: {error}", self.path.display());
        }
    }
}
