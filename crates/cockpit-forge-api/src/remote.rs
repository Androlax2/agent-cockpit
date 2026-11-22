//! Which adapter serves a project's origin (#857, decision 4): an HTTPS URL on a host an
//! adapter declares for Git. An SSH remote, another scheme, a URL carrying a user or a host no
//! adapter declares is refused with a named reason, never served by a lesser path.

use std::fmt;

use crate::declaration::ForgeHost;
use crate::{Forge, ForgeError, ForgeName};

/// Longest repository path.
const MAX_PATH_BYTES: usize = 512;

/// A repository's path on its host, without a leading `/` or a trailing `.git` in any case: one or more
/// names (`owner/name`, `group/subgroup/name`) of ASCII letters, digits, `.`, `_` and `-`,
/// none `.` or `..`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepositoryPath(String);

impl RepositoryPath {
    pub fn new(path: impl Into<String>) -> Result<RepositoryPath, crate::InvalidValue> {
        let path = path.into();
        if !is_repository_path(&path) {
            return Err(crate::InvalidValue {
                kind: "repository path",
                value: path,
                rule: "names separated by `/`, each of ASCII letters, digits, `.`, `_` and `-`, \
                       none empty, `.` or `..`, not ending with `.git`",
            });
        }
        Ok(RepositoryPath(path))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Its names, outermost first.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

impl fmt::Display for RepositoryPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn is_repository_path(path: &str) -> bool {
    path.len() <= MAX_PATH_BYTES
        && !has_git_suffix(path)
        && path.split('/').all(|name| {
            !name.is_empty()
                && name != "."
                && name != ".."
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
        })
}

/// A repository on a forge.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Repository {
    pub host: ForgeHost,
    pub path: RepositoryPath,
}

impl fmt::Display for Repository {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.host, self.path)
    }
}

/// Why an origin has no adapter. Never repeats the URL, which may carry a credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unresolved {
    /// A local path or something else that is not a URL.
    NotAUrl,
    /// `ssh://` or `user@host:path`: no adapter pushes over SSH in the first release.
    Ssh,
    /// A scheme other than `https`.
    Scheme { scheme: String },
    /// The URL names a user or a password; a credential belongs in the owner's token files.
    UserInfo,
    /// An HTTPS URL that does not name a host and a repository path.
    Malformed { reason: String },
    /// No adapter declares this host for Git.
    NoAdapter { host: ForgeHost },
    /// More than one adapter declares this host for Git.
    Ambiguous {
        host: ForgeHost,
        forges: Vec<ForgeName>,
    },
    /// The adapter for this host refused the repository.
    Refused { forge: ForgeName, reason: String },
}

impl fmt::Display for Unresolved {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unresolved::NotAUrl => formatter.write_str("the origin is not a URL"),
            Unresolved::Ssh => formatter.write_str("SSH remotes are not supported yet"),
            Unresolved::Scheme { scheme } => write!(
                formatter,
                "`{scheme}` remotes are not supported: only `https`"
            ),
            Unresolved::UserInfo => formatter.write_str(
                "the origin URL names a user or a password; remove it, a token belongs in \
                 the project's token file",
            ),
            Unresolved::Malformed { reason } => write!(formatter, "the origin URL {reason}"),
            Unresolved::NoAdapter { host } => {
                write!(formatter, "no adapter for this forge: {host}")
            }
            Unresolved::Ambiguous { host, forges } => {
                let names: Vec<&str> = forges.iter().map(ForgeName::as_str).collect();
                write!(
                    formatter,
                    "more than one adapter serves {host}: {}",
                    names.join(", ")
                )
            }
            Unresolved::Refused { forge, reason } => {
                write!(
                    formatter,
                    "the {forge} adapter refuses this origin: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for Unresolved {}

/// The repository an origin URL names: `https://host[:port]/path[.git][/]`, the host
/// lower-cased, `:443` dropped. Refuses every other form with its reason.
pub fn parse_origin(url: &str) -> Result<Repository, Unresolved> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(if is_scp_like(url) {
            Unresolved::Ssh
        } else {
            Unresolved::NotAUrl
        });
    };
    let is_scheme = !scheme.is_empty()
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"+-.".contains(&byte));
    if !is_scheme {
        return Err(Unresolved::NotAUrl);
    }
    let scheme = scheme.to_ascii_lowercase();
    if scheme.split('+').any(|part| part == "ssh") {
        return Err(Unresolved::Ssh);
    }
    if scheme != "https" {
        return Err(Unresolved::Scheme { scheme });
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.contains('@') {
        return Err(Unresolved::UserInfo);
    }
    if url.contains(['?', '#', '%']) || url.chars().any(|character| !character.is_ascii_graphic()) {
        return Err(Unresolved::Malformed {
            reason: "holds a query, a fragment, an escape or a character outside printable \
                     ASCII"
                .to_owned(),
        });
    }
    let authority = authority.to_ascii_lowercase();
    // HTTPS's own port names the same host as none.
    let authority = authority.strip_suffix(":443").unwrap_or(&authority);
    let host = ForgeHost::new(authority).map_err(|invalid| Unresolved::Malformed {
        reason: format!("names no forge host: {}", invalid.rule),
    })?;
    let path = path.strip_suffix('/').unwrap_or(path);
    let path = strip_git_suffix(path);
    let path = RepositoryPath::new(path).map_err(|invalid| Unresolved::Malformed {
        reason: format!("names no repository: {}", invalid.rule),
    })?;
    Ok(Repository { host, path })
}

/// `path` without a trailing `.git`, in any case, as Git hosts read it.
fn strip_git_suffix(path: &str) -> &str {
    if has_git_suffix(path) {
        &path[..path.len() - ".git".len()]
    } else {
        path
    }
}

fn has_git_suffix(path: &str) -> bool {
    path.len() >= ".git".len()
        && path.as_bytes()[path.len() - ".git".len()..].eq_ignore_ascii_case(b".git")
}

/// Git's `[user@]host:path` form, which it reads as SSH: a `:` before any `/`.
fn is_scp_like(url: &str) -> bool {
    match (url.find(':'), url.find('/')) {
        (Some(colon), Some(slash)) => colon > 0 && colon < slash,
        (Some(colon), None) => colon > 0,
        (None, _) => false,
    }
}

/// The adapter that serves `origin`, and the repository it names: the one adapter that
/// declares the origin's host for Git and accepts the repository
/// ([`Forge::check_repository`]).
pub fn resolve<'forge>(
    forges: &[&'forge dyn Forge],
    origin: &str,
) -> Result<(&'forge dyn Forge, Repository), Unresolved> {
    let repository = parse_origin(origin)?;
    let serving: Vec<&'forge dyn Forge> = forges
        .iter()
        .copied()
        .filter(|forge| forge.declaration().git_hosts().contains(&repository.host))
        .collect();
    let forge = match serving.as_slice() {
        [] => {
            return Err(Unresolved::NoAdapter {
                host: repository.host,
            });
        }
        [forge] => *forge,
        _ => {
            return Err(Unresolved::Ambiguous {
                host: repository.host,
                forges: serving.iter().map(|forge| forge.name().clone()).collect(),
            });
        }
    };
    forge
        .check_repository(&repository)
        .map_err(|error| Unresolved::Refused {
            forge: forge.name().clone(),
            reason: match error {
                ForgeError::Refused { reason } => reason,
                other => other.to_string(),
            },
        })?;
    Ok((forge, repository))
}
