//! The hosts a forge adapter reaches and the credentials each takes (#857): what the proxy
//! adds on the wire (#832) and what the daemon pushes with (#155), declared by the adapter so
//! neither needs a forge's name.

use std::collections::BTreeSet;
use std::fmt;

use crate::InvalidValue;
use crate::tokens::TokenKind;

/// Longest host name (RFC 1035).
const MAX_HOST_BYTES: usize = 253;
/// Longest label of a host name (RFC 1035).
const MAX_LABEL_BYTES: usize = 63;

/// A forge's host: a lower-case DNS name of at least two labels with an optional port
/// (`forge.example.com`, `forge.example.com:8443`). No wildcard, no IP address in any form a
/// resolver reads, no `localhost`: a credential is bound to one named host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ForgeHost(String);

impl ForgeHost {
    pub fn new(host: impl Into<String>) -> Result<ForgeHost, InvalidValue> {
        let host = host.into();
        if is_host(&host) {
            Ok(ForgeHost(host))
        } else {
            Err(InvalidValue {
                kind: "forge host",
                value: host,
                rule: "a lower-case DNS name of at least two labels whose last starts with a \
                       letter and is not `localhost`, with an optional port from 1 to 65535; \
                       no wildcard and never an IP address",
            })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ForgeHost {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn is_host(host: &str) -> bool {
    let (name, port) = match host.rsplit_once(':') {
        Some((name, port)) => (name, Some(port)),
        None => (host, None),
    };
    let is_port = port.is_none_or(|port| {
        port.bytes().all(|byte| byte.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|port| port != 0)
    });
    let labels: Vec<&str> = name.split('.').collect();
    // Every top-level domain starts with a letter; a resolver reads a last label starting
    // with a digit (`1`, `0x1`) as part of an IPv4 address.
    let is_public_name = labels.len() >= 2
        && labels.last().is_some_and(|last| {
            last.starts_with(|first: char| first.is_ascii_lowercase()) && *last != "localhost"
        });
    let is_name = name.len() <= MAX_HOST_BYTES && labels.iter().all(|label| is_label(label));
    is_port && is_name && is_public_name
}

fn is_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= MAX_LABEL_BYTES
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// How a token travels in a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CredentialScheme {
    /// `Authorization: Basic`, the token as the password: what Git sends over HTTPS.
    Basic,
    /// `Authorization: Bearer <token>`.
    Bearer,
}

/// What a token is used for on a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HostUse {
    /// Git fetches (`git-upload-pack`) only: the proxy refuses both halves of a push there.
    GitRead,
    /// Git pushes (`git-receive-pack`), made by the daemon alone.
    GitPush,
    /// The forge's API.
    Api,
}

/// One token on one host for one use.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CredentialUse {
    pub token: TokenKind,
    pub host: ForgeHost,
    pub scheme: CredentialScheme,
    pub used_for: HostUse,
}

/// The hosts a forge adapter reaches and the token each takes. Valid by construction:
///
/// - at least one Git host, where a project's origin may point;
/// - the push token is used for Git pushes only, and Git pushes take the push token only, so
///   the token the proxy adds can never write a repository's contents (#857, decision 1);
/// - no use declared twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declaration {
    uses: Vec<CredentialUse>,
}

impl Declaration {
    pub fn new(uses: Vec<CredentialUse>) -> Result<Declaration, InvalidValue> {
        let refuse = |value: String, rule: &'static str| InvalidValue {
            kind: "forge declaration",
            value,
            rule,
        };
        let mut seen = BTreeSet::new();
        for credential in &uses {
            let is_push_token = credential.token == TokenKind::Push;
            let is_push_use = credential.used_for == HostUse::GitPush;
            if is_push_token != is_push_use {
                return Err(refuse(
                    format!("{credential:?}"),
                    "the push token is used for Git pushes only, and Git pushes take the push \
                     token only",
                ));
            }
            if !seen.insert((
                credential.host.clone(),
                credential.token,
                credential.used_for,
            )) {
                return Err(refuse(
                    format!("{credential:?}"),
                    "each token is declared once per host and use",
                ));
            }
        }
        let has_git_host = uses
            .iter()
            .any(|credential| credential.used_for != HostUse::Api);
        if !has_git_host {
            return Err(refuse(
                format!("{uses:?}"),
                "at least one host serves Git, where a project's origin points",
            ));
        }
        Ok(Declaration { uses })
    }

    pub fn uses(&self) -> &[CredentialUse] {
        &self.uses
    }

    /// Every host the adapter reaches: what the sandbox's `reach` right covers.
    pub fn hosts(&self) -> BTreeSet<&ForgeHost> {
        self.uses
            .iter()
            .map(|credential| &credential.host)
            .collect()
    }

    /// The hosts that serve Git: an origin on one of them belongs to this adapter.
    pub fn git_hosts(&self) -> BTreeSet<&ForgeHost> {
        self.uses
            .iter()
            .filter(|credential| credential.used_for != HostUse::Api)
            .map(|credential| &credential.host)
            .collect()
    }

    /// The uses of one kind of token: [`TokenKind::Api`] is what the proxy adds,
    /// [`TokenKind::Push`] what the daemon pushes with.
    pub fn uses_of(&self, token: TokenKind) -> impl Iterator<Item = &CredentialUse> {
        self.uses
            .iter()
            .filter(move |credential| credential.token == token)
    }
}
