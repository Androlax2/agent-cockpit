//! Whether an agent's push changes what CI runs (#155, #857), read against the remote as it
//! is. The checkout is its agent's to write, history and object store alike: a replace ref, a
//! graft or a planted object could make `merge-base` or `diff` there hide a change. So the scan
//! uses no history: it compares what CI reads in the pushed commit with what it reads in the
//! commits the remote's branch and default branch are at. Every object it reads is hashed again
//! and must match its id.

use std::path::Path;
use std::time::Duration;

use crate::Error;
use crate::git::{Git, arg, git_failure};

/// How long one question to the remote may take before Git is killed and the push fails.
const REMOTE_LIMIT: Duration = Duration::from_secs(2 * 60);

/// The folder GitHub Actions reads its workflows from.
pub(crate) const GITHUB_WORKFLOWS: &str = ".github/workflows";
/// The file GitLab CI reads its pipeline from.
pub(crate) const GITLAB_CI: &str = ".gitlab-ci.yml";

/// The commits the remote's branches are at, as `ls-remote` answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteTips {
    /// The branch the remote's `HEAD` names; `None` when it names none.
    pub(crate) default_branch: Option<String>,
    /// The commit the remote's `HEAD` (its default branch) is at; `None` for an empty one.
    pub(crate) default: Option<String>,
    /// The commit the pushed branch is at on the remote; `None` when it is new.
    pub(crate) pushed: Option<String>,
}

/// How a scan reaches the remote: the one URL the push goes to, Git's settings for the call
/// and the one secret it carries.
pub(crate) struct RemoteAccess<'a> {
    pub(crate) git: &'a Git,
    pub(crate) checkout: &'a Path,
    pub(crate) url: &'a str,
    pub(crate) extra_config: &'a [&'a str],
    pub(crate) secret: (&'a str, &'a str),
}

/// Where `ls-remote` runs: outside every repository, so no configuration the checkout's agent
/// writes can rewrite the URL whose tips the scan compares with.
const OUTSIDE_ANY_REPOSITORY: &str = "/";

impl RemoteAccess<'_> {
    fn run(&self, dir: &Path, arguments: &[&std::ffi::OsStr]) -> Result<String, Error> {
        let output = self.git.output_within(
            dir,
            self.extra_config,
            Some(self.secret),
            arguments,
            REMOTE_LIMIT,
        )?;
        if !output.status.success() {
            return Err(git_failure(arguments, &output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// The commits the remote's default branch and `branch` are at.
    pub(crate) fn tips(&self, branch: &str) -> Result<RemoteTips, Error> {
        let pushed_ref = format!("refs/heads/{branch}");
        let listed = self.run(
            Path::new(OUTSIDE_ANY_REPOSITORY),
            &[
                arg("ls-remote"),
                arg("--symref"),
                arg("--end-of-options"),
                arg(self.url),
                arg("HEAD"),
                arg(&pushed_ref),
            ],
        )?;
        Ok(parse_tips(&listed, &pushed_ref))
    }

    /// Fetches the `commits` the checkout lacks, by id: their objects alone, no ref and no
    /// `FETCH_HEAD` written, no maintenance and no submodule. The checkout's configuration
    /// could send the fetch elsewhere, but no other repository can answer for these ids:
    /// every object the scan reads is checked against its id.
    fn fetch_missing(&self, commits: &[&str]) -> Result<(), Error> {
        let mut missing = Vec::new();
        for commit in commits {
            if !is_present(self.git, self.checkout, commit)? {
                missing.push(arg(*commit));
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let mut arguments = vec![
            arg("fetch"),
            arg("--quiet"),
            arg("--no-tags"),
            arg("--no-write-fetch-head"),
            arg("--no-auto-maintenance"),
            arg("--no-recurse-submodules"),
            arg("--end-of-options"),
            arg(self.url),
        ];
        arguments.extend(missing);
        self.run(self.checkout, &arguments)?;
        Ok(())
    }
}

/// Reads `ls-remote`'s answer for `HEAD` and `pushed_ref`.
fn parse_tips(listed: &str, pushed_ref: &str) -> RemoteTips {
    let mut tips = RemoteTips {
        default_branch: None,
        default: None,
        pushed: None,
    };
    for line in listed.lines() {
        let Some((commit, name)) = line.split_once('\t') else {
            continue;
        };
        if let Some(target) = commit.strip_prefix("ref: refs/heads/") {
            if name == "HEAD" {
                tips.default_branch = Some(target.to_owned());
            }
            continue;
        }
        if !is_object_id(commit) {
            continue;
        }
        if name == "HEAD" {
            tips.default = Some(commit.to_owned());
        } else if name == pushed_ref {
            tips.pushed = Some(commit.to_owned());
        }
    }
    tips
}

fn is_object_id(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_present(git: &Git, checkout: &Path, commit: &str) -> Result<bool, Error> {
    let object = format!("{commit}^{{commit}}");
    let output = git.output(
        checkout,
        &[
            arg("cat-file"),
            arg("-e"),
            arg("--end-of-options"),
            arg(&object),
        ],
    )?;
    Ok(output.status.success())
}

/// The CI paths ([`GITHUB_WORKFLOWS`], [`GITLAB_CI`]) whose content in `commit` is neither the
/// content at the remote branch's tip nor at its default branch's: what CI would newly run.
/// A rebase that only brings in the default branch's workflows changes none; on an empty
/// remote, any workflow is new. With no history to read, a branch started before the default
/// branch changed its workflows is held too, until rebased onto it.
pub(crate) fn changed_ci_paths(
    remote: &RemoteAccess<'_>,
    tips: &RemoteTips,
    commit: &str,
) -> Result<Vec<&'static str>, Error> {
    let on_remote: Vec<&str> = [tips.pushed.as_deref(), tips.default.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    remote.fetch_missing(&on_remote)?;
    let read = |commit: &str| ci_entries(remote.git, remote.checkout, commit);
    let pushed = read(commit)?;
    let mut baselines = on_remote
        .iter()
        .map(|commit| read(commit))
        .collect::<Result<Vec<_>, _>>()?;
    if baselines.is_empty() {
        baselines.push(CiEntries::default());
    }
    let mut changed = Vec::new();
    if baselines.iter().all(|known| known.github != pushed.github) {
        changed.push(GITHUB_WORKFLOWS);
    }
    if baselines.iter().all(|known| known.gitlab != pushed.gitlab) {
        changed.push(GITLAB_CI);
    }
    Ok(changed)
}

/// One tree entry: its mode and object id, which name its content exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    mode: Vec<u8>,
    id: String,
}

/// What a commit holds where CI reads: `github` is `.github/workflows`, or `.github` itself
/// when that is not a folder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CiEntries {
    github: Option<Entry>,
    gitlab: Option<Entry>,
}

const TREE_MODE: &[u8] = b"40000";

fn ci_entries(git: &Git, checkout: &Path, commit: &str) -> Result<CiEntries, Error> {
    let content = read_verified(git, checkout, "commit", commit)?;
    let root = root_tree(&content).ok_or_else(|| Error::Refused {
        reason: format!("commit {commit} names no tree"),
    })?;
    let root_entries = tree_entries(&read_verified(git, checkout, "tree", &root)?, &root)?;
    let gitlab = find(&root_entries, GITLAB_CI.as_bytes());
    let github = match find(&root_entries, b".github") {
        Some(folder) if folder.mode == TREE_MODE => {
            let entries = tree_entries(
                &read_verified(git, checkout, "tree", &folder.id)?,
                &folder.id,
            )?;
            find(&entries, b"workflows")
        }
        other => other,
    };
    Ok(CiEntries { github, gitlab })
}

/// The `kind` object `id`'s content, once Git hashed it again to `id`: the checkout's object
/// store, replace refs included, cannot answer for an id with other content.
fn read_verified(git: &Git, checkout: &Path, kind: &str, id: &str) -> Result<Vec<u8>, Error> {
    let arguments = [arg("cat-file"), arg(kind), arg("--end-of-options"), arg(id)];
    let output = git.output(checkout, &arguments)?;
    if !output.status.success() {
        return Err(git_failure(&arguments, &output));
    }
    let hashed = git.run_with_input(
        checkout,
        &[
            arg("hash-object"),
            arg("-t"),
            arg(kind),
            arg("--literally"),
            arg("--stdin"),
        ],
        &output.stdout,
    )?;
    if hashed.trim() != id {
        return Err(Error::Refused {
            reason: format!(
                "the checkout answers for {kind} {id} with other content, so what the push \
                 changes cannot be read; its object store or replace refs were altered"
            ),
        });
    }
    Ok(output.stdout)
}

fn root_tree(commit: &[u8]) -> Option<String> {
    let first_line = commit.split(|byte| *byte == b'\n').next()?;
    let id = std::str::from_utf8(first_line.strip_prefix(b"tree ")?).ok()?;
    is_object_id(id).then(|| id.to_owned())
}

/// A tree's entries, `<mode> <name>\0<raw id>` each, with names kept as bytes.
fn tree_entries(tree: &[u8], tree_id: &str) -> Result<Vec<(Vec<u8>, Entry)>, Error> {
    let id_length = tree_id.len() / 2;
    let malformed = || Error::Refused {
        reason: format!("tree {tree_id} is not one Git wrote"),
    };
    let mut entries = Vec::new();
    let mut rest = tree;
    while !rest.is_empty() {
        let space = rest
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(malformed)?;
        let (mode, after_mode) = (&rest[..space], &rest[space + 1..]);
        let nul = after_mode
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(malformed)?;
        let (name, after_name) = (&after_mode[..nul], &after_mode[nul + 1..]);
        if after_name.len() < id_length {
            return Err(malformed());
        }
        let (raw_id, after_id) = after_name.split_at(id_length);
        let id = raw_id.iter().map(|byte| format!("{byte:02x}")).collect();
        entries.push((
            name.to_vec(),
            Entry {
                mode: mode.to_vec(),
                id,
            },
        ));
        rest = after_id;
    }
    Ok(entries)
}

fn find(entries: &[(Vec<u8>, Entry)], name: &[u8]) -> Option<Entry> {
    entries
        .iter()
        .find(|(entry_name, _)| entry_name == name)
        .map(|(_, entry)| entry.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT: &str = "1111111111111111111111111111111111111111";
    const TIP: &str = "2222222222222222222222222222222222222222";

    #[test]
    fn the_remotes_default_tip_and_the_pushed_tip_are_read_from_ls_remote() {
        let listed =
            format!("ref: refs/heads/main\tHEAD\n{DEFAULT}\tHEAD\n{TIP}\trefs/heads/feature/x\n");

        assert_eq!(
            parse_tips(&listed, "refs/heads/feature/x"),
            RemoteTips {
                default_branch: Some("main".to_owned()),
                default: Some(DEFAULT.to_owned()),
                pushed: Some(TIP.to_owned()),
            }
        );
    }

    #[test]
    fn an_empty_remote_has_no_tips() {
        assert_eq!(
            parse_tips("", "refs/heads/main"),
            RemoteTips {
                default_branch: None,
                default: None,
                pushed: None,
            }
        );
    }

    #[test]
    fn a_tree_whose_last_entry_is_cut_short_is_refused() {
        let mut tree = b"100644 README\0".to_vec();
        tree.extend([0xab; 19]);

        assert!(matches!(
            tree_entries(&tree, DEFAULT),
            Err(Error::Refused { .. })
        ));
    }

    #[test]
    fn a_tree_entry_names_its_mode_and_id() {
        let mut tree = b"40000 .github\0".to_vec();
        tree.extend([0xab; 20]);

        let entries = tree_entries(&tree, DEFAULT).unwrap();

        assert_eq!(
            find(&entries, b".github"),
            Some(Entry {
                mode: TREE_MODE.to_vec(),
                id: "ab".repeat(20),
            })
        );
    }
}
