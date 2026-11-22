//! Issues: a task source beside Torlek's own todos.

use std::fmt;

use crate::{InvalidValue, is_plain_id};

/// Longest issue id; forges number them or use short keys.
const MAX_ISSUE_ID_BYTES: usize = 64;
/// Most issues one list call returns.
pub const MAX_LISTED: usize = 100;

/// The forge's own id of an issue (`153`, `PROJ-12`). Opaque; compared byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IssueId(String);

impl IssueId {
    pub fn new(id: impl Into<String>) -> Result<IssueId, InvalidValue> {
        let id = id.into();
        if !is_plain_id(&id, MAX_ISSUE_ID_BYTES) {
            return Err(InvalidValue {
                kind: "issue id",
                value: id,
                rule: "1 to 64 printable ASCII characters, no spaces",
            });
        }
        Ok(IssueId(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IssueId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IssueState {
    Open,
    Closed,
}

/// An issue as a list shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub id: IssueId,
    pub title: String,
    pub state: IssueState,
    pub labels: Vec<String>,
    /// Who opened it, in the forge's words; `None` when the account is gone.
    pub author: Option<String>,
    /// Where the owner opens it.
    pub url: String,
}

/// An issue with its body and comments, oldest comment first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueThread {
    pub issue: Issue,
    pub body: String,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    /// `None` when the account is gone.
    pub author: Option<String>,
    pub body: String,
}

/// How many issues a list returns: 1 to [`MAX_LISTED`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListLimit(usize);

impl ListLimit {
    pub fn new(limit: usize) -> Result<ListLimit, InvalidValue> {
        if !(1..=MAX_LISTED).contains(&limit) {
            return Err(InvalidValue {
                kind: "list limit",
                value: limit.to_string(),
                rule: "1 to 100",
            });
        }
        Ok(ListLimit(limit))
    }

    pub fn get(self) -> usize {
        self.0
    }
}

/// Which issues a list returns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueFilter {
    /// `None`: open and closed.
    pub state: Option<IssueState>,
    /// Issues carrying every one of these labels.
    pub labels: Vec<String>,
    pub limit: ListLimit,
}

/// An issue to file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewIssue {
    pub title: String,
    pub body: String,
    pub labels: Vec<String>,
}

/// What the forge made: its id and where the owner opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created<Id> {
    pub id: Id,
    pub url: String,
}

/// A comment the forge took, and where the owner reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posted {
    pub url: String,
}
