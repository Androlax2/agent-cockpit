//! A project's repository tokens (#857): the push token the daemon alone reads, and the API
//! token the proxy adds. A scope check says whether a token has what its kind needs and
//! nothing its kind must not have.

use std::fmt;

use crate::InvalidValue;

/// Longest token accepted; far above any forge's real one.
const MAX_TOKEN_BYTES: usize = 1024;

/// A token's value. It is never printed: `Debug` hides it and there is no `Display`, so it
/// cannot reach a log, an error or a reply by accident.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// Refuses an empty value and one with a space, a control character or a non-ASCII
    /// character, without repeating it.
    pub fn new(value: impl Into<String>) -> Result<Token, InvalidValue> {
        let value = value.into();
        let is_plain = !value.is_empty()
            && value.len() <= MAX_TOKEN_BYTES
            && value.chars().all(|character| character.is_ascii_graphic());
        if !is_plain {
            return Err(InvalidValue {
                kind: "token",
                value: "<hidden>".to_owned(),
                rule: "1 to 1024 printable ASCII characters, no spaces",
            });
        }
        Ok(Token(value))
    }

    /// The value, for the one place that sends it to the forge.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Token(<hidden>)")
    }
}

/// Which of a project's two tokens (#857).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TokenKind {
    /// Writes the repository's contents; read by the daemon only, for `torlek push`.
    Push,
    /// Writes change requests and issues, reads contents; added by the proxy on the forge's
    /// API and Git-read hosts.
    Api,
}

impl TokenKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Push => "push",
            TokenKind::Api => "api",
        }
    }

    /// What a token of this kind must be able to do.
    pub fn required(self) -> &'static [Permission] {
        match self {
            TokenKind::Push => &[Permission::ContentsWrite],
            TokenKind::Api => &[
                Permission::ContentsRead,
                Permission::ChangeRequestsWrite,
                Permission::IssuesWrite,
            ],
        }
    }

    /// What a token of this kind must not be able to do.
    pub fn forbidden(self) -> &'static [Permission] {
        match self {
            TokenKind::Push => &[Permission::Workflows, Permission::Administration],
            TokenKind::Api => &[
                Permission::ContentsWrite,
                Permission::Workflows,
                Permission::Administration,
            ],
        }
    }
}

impl fmt::Display for TokenKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A permission a repository token may carry, in forge-neutral words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Permission {
    ContentsRead,
    ContentsWrite,
    ChangeRequestsWrite,
    IssuesWrite,
    /// Change the forge's CI definitions.
    Workflows,
    /// Change the repository's settings, protection included.
    Administration,
}

impl Permission {
    pub fn as_str(self) -> &'static str {
        match self {
            Permission::ContentsRead => "contents read",
            Permission::ContentsWrite => "contents write",
            Permission::ChangeRequestsWrite => "change requests write",
            Permission::IssuesWrite => "issues write",
            Permission::Workflows => "workflows",
            Permission::Administration => "administration",
        }
    }
}

impl fmt::Display for Permission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The answer of a scope check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeCheck {
    /// The forge showed the token has every required permission, none forbidden, on this
    /// repository alone.
    Verified,
    /// The forge does not show enough to tell; `reason` says what it could not see.
    Unverified { reason: String },
    /// The token is wrong for its kind.
    Refused { problems: Vec<ScopeProblem> },
}

/// One way a token is wrong for its kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeProblem {
    Missing(Permission),
    Excess(Permission),
    /// It reaches repositories beyond the project's.
    OtherRepositories,
    /// The forge does not accept it: revoked, expired or mistyped.
    NotAccepted,
}

impl fmt::Display for ScopeProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScopeProblem::Missing(permission) => write!(formatter, "it lacks {permission}"),
            ScopeProblem::Excess(permission) => write!(formatter, "it has {permission}"),
            ScopeProblem::OtherRepositories => formatter.write_str("it reaches other repositories"),
            ScopeProblem::NotAccepted => formatter.write_str("the forge does not accept it"),
        }
    }
}
