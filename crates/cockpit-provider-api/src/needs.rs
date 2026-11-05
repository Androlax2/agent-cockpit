//! What a program declares it needs from the sandbox: the hosts it reaches and the folders of
//! the home it reads and writes (#830, #861), and for an adapter the credentials it takes
//! (#858). A declaration requests; the sandbox (#829, #831, #832) decides what backs it and
//! refuses what its own rules forbid.

use std::fmt;

use crate::InvalidValue;

/// Longest host name (RFC 1035).
const MAX_HOST_BYTES: usize = 253;
/// Longest label of a host name (RFC 1035).
const MAX_LABEL_BYTES: usize = 63;

/// A host a program reaches: a lower-case DNS name with an optional port (`api.example.com`,
/// `api.example.com:8443`), or every name under a domain (`*.example.com`). A bare `*`, a
/// wildcard on a single label (`*.com`), an IP literal and a port of 0 are refused: the proxy
/// allows names, never addresses (#861).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Host(String);

impl Host {
    pub fn new(host: impl Into<String>) -> Result<Host, InvalidValue> {
        let host = host.into();
        if is_host(&host) {
            Ok(Host(host))
        } else {
            Err(InvalidValue {
                kind: "host",
                value: host,
                rule: "a lower-case DNS name, or `*.` and a domain of at least two labels, with \
                       an optional port from 1 to 65535; never an IP address",
            })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Host {
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
    let (domain, is_wildcard) = match name.strip_prefix("*.") {
        Some(domain) => (domain, true),
        None => (name, false),
    };
    let labels: Vec<&str> = domain.split('.').collect();
    let is_name = name.len() <= MAX_HOST_BYTES
        && labels.iter().all(|label| is_label(label))
        // An all-digit last label is an IPv4 address, never a top-level domain.
        && labels
            .last()
            .is_some_and(|last| !last.bytes().all(|byte| byte.is_ascii_digit()));
    let is_narrow_enough = !is_wildcard || labels.len() >= 2;
    is_port && is_name && is_narrow_enough
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

/// A path in the agent's home, written `~/` and its components (`~/.claude`,
/// `~/.claude.json.*`). Its last component may hold `*`, matching any run of characters in one
/// name; no component is empty, `.`, `..` or `*` alone. The sandbox expands `~` to the home
/// the agent runs with.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HomePath(String);

impl HomePath {
    pub fn new(path: impl Into<String>) -> Result<HomePath, InvalidValue> {
        let path = path.into();
        if is_home_path(&path) {
            Ok(HomePath(path))
        } else {
            Err(InvalidValue {
                kind: "home path",
                value: path,
                rule: "`~/` and names, none empty, `.`, `..` or `*` alone; `*` only in the last \
                       name",
            })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Its names after `~/`.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0["~/".len()..].split('/')
    }

    /// Whether `other` is this path or lies inside it, so that binding this path exposes
    /// `other`. A `*` in this path's last name matches any run of characters there.
    pub fn covers(&self, other: &HomePath) -> bool {
        let mut others = other.components();
        self.components().all(|name| {
            others
                .next()
                .is_some_and(|other_name| matches_name(name, other_name))
        })
    }
}

impl fmt::Display for HomePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn is_home_path(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("~/") else {
        return false;
    };
    let names: Vec<&str> = rest.split('/').collect();
    let last = names.len() - 1;
    names.iter().enumerate().all(|(index, name)| {
        !name.is_empty()
            && *name != "."
            && *name != ".."
            && *name != "*"
            && !name.contains('\0')
            && (index == last || !name.contains('*'))
    })
}

/// Whether `name` matches `pattern`, whose `*` match any run of characters.
pub(crate) fn matches_name(pattern: &str, name: &str) -> bool {
    let mut pieces = pattern.split('*');
    let first = pieces.next().unwrap_or_default();
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let pieces: Vec<&str> = pieces.collect();
    let Some((last, middle)) = pieces.split_last() else {
        return rest.is_empty();
    };
    for piece in middle {
        match rest.find(piece) {
            Some(start) => rest = &rest[start + piece.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

/// An environment variable's name: ASCII letters, digits and `_`, not starting with a digit.
fn is_variable(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(|first: char| first.is_ascii_digit())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

/// What a program declares it needs (#861): the one shape an adapter (#830) and the owner's
/// configuration entry for an unadapted program (#84, #294) both produce. A project file's
/// hosts are requests the owner decides (#674), never part of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Needs {
    /// The hosts it reaches.
    pub hosts: Vec<Host>,
    /// Paths of the owner's home it reads, bound read-only: its installed program, never a
    /// credential ([`Credentials::files`]).
    pub read: Vec<HomePath>,
    /// Paths it writes. What backs them is the sandbox's, by source: for an adapter, a folder
    /// Torlek owns for the session, kept across its relaunches and never the owner's (#858,
    /// #859); for the owner's entry, the owner's own folder (#861).
    pub write: Vec<HomePath>,
}

impl Needs {
    /// The floor: no host and no path beyond the checkout and the session's own folders.
    pub fn none() -> Needs {
        Needs::default()
    }

    /// A declaration from its written form, refusing the first malformed value.
    pub fn new(hosts: &[&str], read: &[&str], write: &[&str]) -> Result<Needs, InvalidValue> {
        Ok(Needs {
            hosts: hosts
                .iter()
                .copied()
                .map(Host::new)
                .collect::<Result<_, _>>()?,
            read: home_paths(read)?,
            write: home_paths(write)?,
        })
    }
}

fn home_paths(paths: &[&str]) -> Result<Vec<HomePath>, InvalidValue> {
    paths.iter().copied().map(HomePath::new).collect()
}

/// One credential a harness takes from its environment, and the hosts that carry it. The
/// session holds a placeholder in `variable`; the proxy adds the real value on `hosts` only
/// (#832, #858).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialBinding {
    pub variable: String,
    pub hosts: Vec<Host>,
}

/// Where an adapter's harness keeps and takes its credentials (#858).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Credentials {
    /// Files where the harness stores a sign-in. Never bound from the owner's home: no
    /// [`Needs::read`] path covers one. One may lie under a [`Needs::write`] path, which the
    /// session's own folder backs.
    pub files: Vec<HomePath>,
    /// The variables that take a bound credential, each with the hosts that carry it. They are
    /// alternatives: a launch binds one credential per host.
    pub bindings: Vec<CredentialBinding>,
}

impl Credentials {
    /// A program that declares no credential.
    pub fn none() -> Credentials {
        Credentials::default()
    }

    /// A declaration from its written form, checked against the program's `needs`: every
    /// credential host is one of its hosts, and no path it reads from the owner's home
    /// covers a credential file.
    pub fn new(
        files: &[&str],
        bindings: &[(&str, &[&str])],
        needs: &Needs,
    ) -> Result<Credentials, InvalidValue> {
        let files = home_paths(files)?;
        if let Some(exposed) = files
            .iter()
            .find(|file| needs.read.iter().any(|read| read.covers(file)))
        {
            return Err(InvalidValue {
                kind: "credential file",
                value: exposed.to_string(),
                rule: "no path read from the owner's home covers a credential file",
            });
        }
        let bindings = bindings
            .iter()
            .map(|(variable, hosts)| credential_binding(variable, hosts, needs))
            .collect::<Result<_, _>>()?;
        Ok(Credentials { files, bindings })
    }
}

fn credential_binding(
    variable: &str,
    hosts: &[&str],
    needs: &Needs,
) -> Result<CredentialBinding, InvalidValue> {
    if !is_variable(variable) {
        return Err(InvalidValue {
            kind: "credential variable",
            value: variable.to_owned(),
            rule: "ASCII letters, digits and `_`, not starting with a digit",
        });
    }
    let hosts: Vec<Host> = hosts
        .iter()
        .copied()
        .map(Host::new)
        .collect::<Result<_, _>>()?;
    if let Some(undeclared) = hosts.iter().find(|host| !needs.hosts.contains(host)) {
        return Err(InvalidValue {
            kind: "credential host",
            value: undeclared.to_string(),
            rule: "a credential host is one of the program's declared hosts",
        });
    }
    if hosts.is_empty() {
        return Err(InvalidValue {
            kind: "credential variable",
            value: variable.to_owned(),
            rule: "a credential is carried by at least one host",
        });
    }
    Ok(CredentialBinding {
        variable: variable.to_owned(),
        hosts,
    })
}
