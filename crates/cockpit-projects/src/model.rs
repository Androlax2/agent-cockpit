use std::fmt;
use std::path::PathBuf;

use crate::Error;

/// Longest project name; it is a directory name under the clones directory.
const MAX_NAME_BYTES: usize = 64;
/// Longest holder id; holders are session ids, far shorter in practice.
const MAX_HOLDER_BYTES: usize = 256;

/// A project's unique name. It names the project's clones directory, so it is one plain path
/// component: ASCII letters, digits, `.`, `_` and `-`, not starting with `.` or `-`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ProjectName(String);

impl ProjectName {
    pub fn new(name: impl Into<String>) -> Result<ProjectName, Error> {
        let name = name.into();
        let invalid = |reason: &str| {
            Err(Error::InvalidName {
                name: name.clone(),
                reason: reason.to_owned(),
            })
        };
        if name.is_empty() || name.len() > MAX_NAME_BYTES {
            return invalid("a project name has 1 to 64 characters");
        }
        if name.starts_with(['.', '-']) {
            return invalid("a project name does not start with `.` or `-`");
        }
        let is_plain = |character: char| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
        };
        if !name.chars().all(is_plain) {
            return invalid("a project name uses only ASCII letters, digits, `.`, `_` and `-`");
        }
        Ok(ProjectName(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProjectId(pub i64);

/// One incarnation of a working copy. Ids are never reused: a checkout recreated at the same
/// path is a new id with the next incarnation (report 04: the path is a locator, not identity).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CheckoutId(pub i64);

impl fmt::Display for CheckoutId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Who occupies a checkout: a session id as the sessions capability issued it. Opaque and
/// compared byte for byte.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Holder(String);

impl Holder {
    pub fn new(id: impl Into<String>) -> Result<Holder, Error> {
        let id = id.into();
        if id.is_empty() || id.len() > MAX_HOLDER_BYTES || id.chars().any(char::is_control) {
            return Err(Error::InvalidHolder { holder: id });
        }
        Ok(Holder(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Holder {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: ProjectName,
    /// The main checkout's canonical path.
    pub root: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckoutKind {
    /// The checkout the owner registered.
    Main,
    /// A parallel clone working on `branch`; it inherits the project's trust and hooks.
    Parallel { branch: String },
    /// A clone of a pull request's head for review; never trusted, no hooks installed.
    Review { pull_request: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckoutState {
    Live,
    /// The sweep refuses claims while it checks the clone for work; an interrupted check is
    /// done again by the next sweep.
    Removing,
    /// The sweep's checks passed and deletion started; the next sweep finishes it.
    Deleting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Occupancy {
    pub holder: Holder,
    /// Seconds since the Unix epoch.
    pub claimed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub id: CheckoutId,
    pub project: ProjectName,
    pub project_root: PathBuf,
    pub kind: CheckoutKind,
    pub path: PathBuf,
    /// 1 for the first working copy at `path`, counting up each time one is recreated there.
    pub incarnation: i64,
    pub state: CheckoutState,
    pub occupancy: Option<Occupancy>,
}

/// Whose trust decision covers a checkout (`projects.trusted`, report 09).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// The owner trusts this checkout's own path.
    Trusted,
    /// A parallel clone of a trusted project inherits the main checkout's trust.
    Inherited {
        from: PathBuf,
    },
    Untrusted,
    /// Review clones hold someone else's changes: never trusted, whatever the project is.
    NeverForReview,
}
