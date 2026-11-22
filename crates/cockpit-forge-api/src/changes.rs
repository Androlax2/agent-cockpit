//! Change requests (a forge's pull or merge requests): verification evidence for tasks
//! (report 06). Merged, checks passed and review approved are separate facts, each read on
//! its own.

use std::fmt;

use crate::{InvalidValue, is_plain_id};

/// Longest change request id.
const MAX_CHANGE_ID_BYTES: usize = 64;
/// Longest branch name Torlek accepts.
const MAX_BRANCH_BYTES: usize = 255;

/// The forge's own id of a change request (`1169`). Opaque; compared byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChangeId(String);

impl ChangeId {
    pub fn new(id: impl Into<String>) -> Result<ChangeId, InvalidValue> {
        let id = id.into();
        if !is_plain_id(&id, MAX_CHANGE_ID_BYTES) {
            return Err(InvalidValue {
                kind: "change request id",
                value: id,
                rule: "1 to 64 ASCII letters, digits, `.`, `_` and `-`, not `.` or `..`",
            });
        }
        Ok(ChangeId(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ChangeId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A branch name, without `refs/heads/`: printable ASCII with no space and none of the
/// characters Git refuses in a reference (`~^:?*[\`), not starting with `-` or `/`, with no
/// `..`, `//` or `@{`, not ending with `/` or `.`, no part starting with `.` or ending with
/// `.lock`, and not `HEAD`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BranchName(String);

impl BranchName {
    pub fn new(name: impl Into<String>) -> Result<BranchName, InvalidValue> {
        let name = name.into();
        if !is_branch(&name) {
            return Err(InvalidValue {
                kind: "branch name",
                value: name,
                rule: "a Git branch name without `refs/heads/`: printable ASCII, no space or \
                       `~^:?*[\\`, no `..`, `//` or `@{`, not starting with `-` or `/`, not \
                       ending with `/`, `.` or `.lock`",
            });
        }
        Ok(BranchName(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BranchName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn is_branch(name: &str) -> bool {
    let has_allowed_characters = name
        .chars()
        .all(|character| character.is_ascii_graphic() && !"~^:?*[\\".contains(character));
    let has_refused_component = name
        .split('/')
        .any(|part| part.starts_with('.') || part.ends_with(".lock"));
    !name.is_empty()
        && name.len() <= MAX_BRANCH_BYTES
        && has_allowed_characters
        && !has_refused_component
        && !name.starts_with(['-', '/'])
        && !name.ends_with(['/', '.'])
        && !name.contains("..")
        && !name.contains("//")
        && !name.contains("@{")
        && name != "@"
        && name != "HEAD"
}

/// A commit's full object id: 40 (SHA-1) or 64 (SHA-256) lower-case hexadecimal digits.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommitId(String);

impl CommitId {
    pub fn new(id: impl Into<String>) -> Result<CommitId, InvalidValue> {
        let id = id.into();
        let is_full = matches!(id.len(), 40 | 64)
            && id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !is_full {
            return Err(InvalidValue {
                kind: "commit id",
                value: id,
                rule: "40 or 64 lower-case hexadecimal digits",
            });
        }
        Ok(CommitId(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeState {
    Open,
    /// Closed without merging.
    Closed,
    /// Merged; `commit` is the commit the merge made or fast-forwarded to, when the forge
    /// names it.
    Merged {
        commit: Option<CommitId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeRequest {
    pub id: ChangeId,
    pub title: String,
    pub url: String,
    pub state: ChangeState,
    pub draft: bool,
    /// The branch it merges from.
    pub head: BranchName,
    /// The commit its head is at: the exact result its checks and review are about.
    pub head_commit: CommitId,
    /// The branch it merges into.
    pub base: BranchName,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckState {
    /// Queued or running.
    Pending,
    Passed,
    Failed,
    /// Did not run, by the forge's own rule (skipped, neutral).
    Skipped,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// The check's name, in the forge's words.
    pub name: String,
    pub state: CheckState,
    /// Whether the base branch's protection requires it; `None` when the forge does not say.
    pub required: Option<bool>,
}

/// The checks on one commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checks {
    pub commit: CommitId,
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ReviewDecision {
    Approved,
    ChangesRequested,
    /// A review is required and none approved yet.
    Required,
    /// The base branch requires no review.
    NotRequired,
}

/// A change request to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewChangeRequest {
    pub title: String,
    pub body: String,
    pub head: BranchName,
    pub base: BranchName,
    pub draft: bool,
}
