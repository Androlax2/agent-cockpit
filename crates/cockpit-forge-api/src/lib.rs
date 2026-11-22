#![doc = include_str!("../README.md")]

mod capabilities;
mod changes;
#[cfg(feature = "conformance")]
pub mod conformance;
mod declaration;
#[cfg(feature = "fake")]
pub mod fake;
mod issues;
mod remote;
mod tokens;

use std::fmt;

pub use capabilities::{Availability, Capabilities, Capability, Provenance};
pub use changes::{
    BranchName, ChangeId, ChangeRequest, ChangeState, Check, CheckState, Checks, CommitId,
    NewChangeRequest, ReviewDecision,
};
pub use declaration::{CredentialScheme, CredentialUse, Declaration, ForgeHost, HostUse};
pub use issues::{
    Comment, Created, Issue, IssueFilter, IssueId, IssueState, IssueThread, ListLimit, MAX_LISTED,
    NewIssue, Posted,
};
pub use remote::{Repository, RepositoryPath, Unresolved, parse_origin, resolve};
pub use tokens::{Permission, ScopeCheck, ScopeProblem, Token, TokenKind};

/// Longest forge name; it is typed on the command line.
const MAX_NAME_BYTES: usize = 32;

/// A forge adapter's name, as the owner types it: lower-case ASCII letters, digits and `-`,
/// starting with a letter.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ForgeName(String);

impl ForgeName {
    pub fn new(name: impl Into<String>) -> Result<ForgeName, InvalidValue> {
        let name = name.into();
        let is_plain = name.len() <= MAX_NAME_BYTES
            && name.starts_with(|first: char| first.is_ascii_lowercase())
            && name.chars().all(|character| {
                character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
            });
        if !is_plain {
            return Err(InvalidValue {
                kind: "forge name",
                value: name,
                rule: "1 to 32 lower-case ASCII letters, digits and `-`, starting with a letter",
            });
        }
        Ok(ForgeName(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ForgeName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a call needs to reach the forge: the repository it is about and the token it is
/// made with. The caller picks the token ([`TokenKind`]); the adapter never reads one itself.
#[derive(Debug, Clone, Copy)]
pub struct Access<'access> {
    pub repository: &'access Repository,
    pub token: &'access Token,
}

/// A code forge's adapter, hosted or a plain Git remote. Nothing above this contract
/// is keyed on a forge's name, only on its reported [`Capabilities`] and its [`Declaration`].
///
/// Every call reaches the forge over the network or through its command-line program, so it
/// blocks: callers run it off their async workers. Text a call returns (titles, bodies,
/// comments, check names) is written by whoever can write on the forge, never by the owner
/// or Torlek. An error never carries the token.
pub trait Forge: Send + Sync + 'static {
    fn name(&self) -> &ForgeName;

    /// The version of the program or API the adapter talks to, when it discovered it.
    fn version(&self) -> Option<&str> {
        None
    }

    fn capabilities(&self) -> Capabilities;

    /// The hosts it reaches, and which token goes to each by which scheme (#857). The proxy
    /// and the daemon's push read this, never a forge's name.
    fn declaration(&self) -> &Declaration;

    /// Refuses a repository on one of its declared Git hosts that it still cannot serve,
    /// such as a path of the wrong shape, naming why. [`resolve`] asks it. The default
    /// accepts every path.
    fn check_repository(&self, repository: &Repository) -> Result<(), ForgeError> {
        let _ = repository;
        Ok(())
    }

    /// Issues matching `filter`, at most its limit, newest first
    /// ([`Capability::IssueList`]).
    fn list_issues(
        &self,
        access: &Access<'_>,
        filter: &IssueFilter,
    ) -> Result<Vec<Issue>, ForgeError> {
        let _ = (access, filter);
        Err(ForgeError::unsupported(Capability::IssueList))
    }

    /// One issue with its body and comments, oldest comment first
    /// ([`Capability::IssueRead`]). [`ForgeError::NotFound`] when it does not exist.
    fn read_issue(&self, access: &Access<'_>, id: &IssueId) -> Result<IssueThread, ForgeError> {
        let _ = (access, id);
        Err(ForgeError::unsupported(Capability::IssueRead))
    }

    /// Files an issue ([`Capability::IssueCreate`]).
    fn create_issue(
        &self,
        access: &Access<'_>,
        issue: &NewIssue,
    ) -> Result<Created<IssueId>, ForgeError> {
        let _ = (access, issue);
        Err(ForgeError::unsupported(Capability::IssueCreate))
    }

    /// Comments on an issue ([`Capability::IssueComment`]).
    fn comment_on_issue(
        &self,
        access: &Access<'_>,
        id: &IssueId,
        body: &str,
    ) -> Result<Posted, ForgeError> {
        let _ = (access, id, body);
        Err(ForgeError::unsupported(Capability::IssueComment))
    }

    /// A change request's state, head and base ([`Capability::ChangeState`]).
    /// [`ForgeError::NotFound`] when it does not exist.
    fn change_request(
        &self,
        access: &Access<'_>,
        id: &ChangeId,
    ) -> Result<ChangeRequest, ForgeError> {
        let _ = (access, id);
        Err(ForgeError::unsupported(Capability::ChangeState))
    }

    /// The newest change request whose head is `branch` in the repository, open or not
    /// ([`Capability::ChangeState`]); `None` when there is none.
    fn change_request_for(
        &self,
        access: &Access<'_>,
        branch: &BranchName,
    ) -> Result<Option<ChangeRequest>, ForgeError> {
        let _ = (access, branch);
        Err(ForgeError::unsupported(Capability::ChangeState))
    }

    /// The checks of a change request's head commit, and that commit
    /// ([`Capability::ChangeChecks`]).
    fn change_checks(&self, access: &Access<'_>, id: &ChangeId) -> Result<Checks, ForgeError> {
        let _ = (access, id);
        Err(ForgeError::unsupported(Capability::ChangeChecks))
    }

    /// Where a change request's review stands ([`Capability::ChangeReview`]).
    fn change_review(
        &self,
        access: &Access<'_>,
        id: &ChangeId,
    ) -> Result<ReviewDecision, ForgeError> {
        let _ = (access, id);
        Err(ForgeError::unsupported(Capability::ChangeReview))
    }

    /// Opens a change request ([`Capability::ChangeOpen`]).
    fn open_change_request(
        &self,
        access: &Access<'_>,
        request: &NewChangeRequest,
    ) -> Result<Created<ChangeId>, ForgeError> {
        let _ = (access, request);
        Err(ForgeError::unsupported(Capability::ChangeOpen))
    }

    /// Whether `access.token` has what a token of `kind` must have on `access.repository`
    /// and nothing it must not ([`Capability::TokenScope`]). Where the forge cannot tell,
    /// [`ScopeCheck::Unverified`] says so; it is never reported verified.
    fn check_token(&self, access: &Access<'_>, kind: TokenKind) -> Result<ScopeCheck, ForgeError> {
        let _ = (access, kind);
        Err(ForgeError::unsupported(Capability::TokenScope))
    }
}

/// Why a forge call gave no answer. None carries a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForgeError {
    /// The adapter does not report `capability`: a typed answer, not a failure.
    Unsupported { capability: Capability },
    /// The adapter does not report `capability`, for `reason`.
    Withheld {
        capability: Capability,
        reason: String,
    },
    /// The forge has no such issue, change request or repository, or the token cannot see it.
    NotFound { what: String },
    /// The forge refused the call: a permission the token lacks, a rejected token, a request
    /// it does not accept.
    Refused { reason: String },
    /// The forge or its program could not be reached or did not answer in time.
    Unavailable { reason: String },
    /// The forge answered something the adapter cannot read.
    Malformed { reason: String },
}

impl ForgeError {
    /// An operation that needs `capability`, which the adapter does not report.
    pub fn unsupported(capability: Capability) -> ForgeError {
        ForgeError::Unsupported { capability }
    }

    /// Whether this is the typed answer that the capability is not there, rather than a
    /// failed call.
    pub fn is_unsupported(&self) -> bool {
        matches!(
            self,
            ForgeError::Unsupported { .. } | ForgeError::Withheld { .. }
        )
    }
}

impl fmt::Display for ForgeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ForgeError::Unsupported { capability } => write!(
                formatter,
                "the forge adapter does not report the `{capability}` capability"
            ),
            ForgeError::Withheld { capability, reason } => write!(
                formatter,
                "the forge adapter does not report the `{capability}` capability: {reason}"
            ),
            ForgeError::NotFound { what } => write!(formatter, "the forge has no {what}"),
            ForgeError::Refused { reason } => write!(formatter, "the forge refused: {reason}"),
            ForgeError::Unavailable { reason } => {
                write!(formatter, "the forge is unavailable: {reason}")
            }
            ForgeError::Malformed { reason } => {
                write!(formatter, "the forge's answer is unreadable: {reason}")
            }
        }
    }
}

impl std::error::Error for ForgeError {}

/// A name, id or declaration that breaks its rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidValue {
    pub kind: &'static str,
    pub value: String,
    pub rule: &'static str,
}

impl fmt::Display for InvalidValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?} is not a valid {}: {}",
            self.value, self.kind, self.rule
        )
    }
}

impl std::error::Error for InvalidValue {}

/// A short, printable identifier the forge gave: no spaces, no control characters.
pub(crate) fn is_plain_id(id: &str, max_bytes: usize) -> bool {
    !id.is_empty()
        && id.len() <= max_bytes
        && id.chars().all(|character| character.is_ascii_graphic())
}
