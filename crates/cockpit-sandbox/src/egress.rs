//! The egress decision core (#831): whether one connection may leave the sandbox, from an
//! allowlist snapshot alone. A session's launcher is its first listener; a project's
//! containers (#856), a check run and a plugin (#862) reuse it, so nothing here is
//! session-only: the subject a listener serves travels beside the snapshot, in the spec.
//!
//! Names are allowed by entry. Every address a name resolves to must be public, unless an
//! entry grants that address and port: loopback, private, link-local (cloud metadata),
//! shared, multicast and reserved ranges are refused, which also closes DNS rebinding. An IP
//! literal is refused unless granted by address. A name on port 443 must present the same
//! name as its TLS server name; any other granted port is a plain tunnel.

use std::collections::BTreeMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use serde::{Deserialize, Serialize};

use crate::spec::Subject;

/// Where the session proxy listens inside the sandbox's network namespace: each namespace
/// has its own loopback, so the fixed port is the session's alone.
pub const PROXY_ADDRESS: &str = "127.0.0.1:3128";

/// The port whose tunnels must name their host in the TLS ClientHello.
pub const TLS_PORT: u16 = 443;

/// The file beside the launch spec, in the session's mount folder no agent reaches, where the
/// launcher keeps its [`EgressReport`] current.
pub const EGRESS_FILE: &str = "egress.json";

/// The most destinations a report names; the rest add up in [`EgressReport::other`].
pub const MAX_REPORTED_DESTINATIONS: usize = 256;

/// The file beside the launch spec where the daemon keeps the owner's decisions on the
/// subject's network requests (#674): the launcher reads it before serving and again each
/// time it changes, so a decision applies to the next connection and a revocation closes the
/// tunnels it allowed.
pub const DECISIONS_FILE: &str = "egress-decisions.json";

/// The file beside the launch spec where the launcher lists the destinations it refused for
/// want of an entry; the daemon files each as a network request the owner decides (#674).
/// Only a file of the session's mount folder, which no agent reaches: an agent cannot ask
/// for a host except by trying it.
pub const REQUESTS_FILE: &str = "egress-requests.json";

/// The most destinations the launcher lists in [`REQUESTS_FILE`]; past it, a refused
/// destination is refused without asking.
pub const MAX_ASKED_DESTINATIONS: usize = 256;

/// The owner's decisions on a subject's network requests, as the daemon writes them to
/// [`DECISIONS_FILE`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressDecisions {
    /// What the owner allowed: for this session ([`AllowSource::Request`]) or always for its
    /// project ([`AllowSource::Grant`]).
    pub allowed: Vec<AllowEntry>,
    /// What the owner denied this subject.
    pub denied: Vec<DeniedDestination>,
}

/// A destination the owner denied, and the request denied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeniedDestination {
    pub host: String,
    pub port: u16,
    pub request: i64,
}

impl EgressDecisions {
    /// The request whose denial covers `host` on `port`, if the owner denied it.
    pub fn denial(&self, host: &str, port: u16) -> Option<i64> {
        self.denied
            .iter()
            .find(|denied| denied.port == port && denied.host == host)
            .map(|denied| denied.request)
    }

    /// Whether the owner decided on `host` on `port`: allowed by name or denied.
    pub fn covers(&self, host: &str, port: u16) -> bool {
        self.denial(host, port).is_some()
            || self.allowed.iter().any(|entry| {
                entry.port == port && matches!(&entry.target, Target::Name(name) if name == host)
            })
    }
}

/// The destinations a launcher refused for want of an entry, as it writes them to
/// [`REQUESTS_FILE`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressRequests {
    pub subject: Subject,
    /// Increases with each write, across the session's launches: a destination asked about
    /// again after a decision on it was taken back makes the file differ from the one the
    /// daemon last read. Absent from a launcher before it: 0.
    #[serde(default)]
    pub revision: u64,
    pub asked: Vec<AskedDestination>,
}

/// Whether a refused `host` on `port` is asked of the owner: a lower-case DNS name of at least
/// two labels, as an entry may name it. The launcher says the owner is asked only of these,
/// and the daemon files only these.
pub fn is_askable_destination(host: &str, port: u16) -> bool {
    port != 0 && host.contains('.') && normalized_name(host).as_deref() == Some(host)
}

/// One refused destination: a lower-case DNS name and a port, as the `CONNECT` named them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AskedDestination {
    pub host: String,
    pub port: u16,
}

/// What one listener let out and refused, per destination (`host:port`): bounded counters the
/// evidence shows and #718 builds on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressReport {
    pub subject: Subject,
    pub destinations: BTreeMap<String, Counters>,
    /// Destinations past [`MAX_REPORTED_DESTINATIONS`].
    pub other: Counters,
}

/// One destination's counters. A refusal is policy's answer (`403`); an unreachable upstream
/// is the network's (`502`, `504`), never counted as a refusal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counters {
    /// The entry's source, for a destination an entry allows.
    pub source: Option<String>,
    pub allowed: u64,
    pub refused: u64,
    pub unreachable: u64,
    pub bytes_out: u64,
    pub bytes_in: u64,
    /// Requests the bound credential was added to (#832).
    #[serde(default)]
    pub credential_added: u64,
    /// Connections whose client refused the per-session CA's certificate (#832), as a client
    /// that pins its certificates does.
    #[serde(default)]
    pub ca_refused: u64,
    /// Why the last connection was refused or failed.
    pub last_reason: Option<String>,
}

impl Counters {
    /// One line for the evidence.
    pub fn describe(&self) -> String {
        let mut line = format!(
            "allowed {}, refused {}, unreachable {}, {} bytes out, {} bytes in",
            self.allowed, self.refused, self.unreachable, self.bytes_out, self.bytes_in
        );
        if self.credential_added > 0 {
            line.push_str(&format!(
                ", credential added to {} requests",
                self.credential_added
            ));
        }
        if self.ca_refused > 0 {
            line.push_str(&format!(
                ", session CA refused by the client {} times",
                self.ca_refused
            ));
        }
        if let Some(source) = &self.source {
            line = format!("({source}) {line}");
        }
        if let Some(reason) = &self.last_reason {
            line.push_str(&format!("; last: {reason}"));
        }
        line
    }
}

impl EgressReport {
    pub fn new(subject: Subject) -> EgressReport {
        EgressReport {
            subject,
            destinations: BTreeMap::new(),
            other: Counters::default(),
        }
    }

    /// The counters of `destination`, or of [`EgressReport::other`] once the report is full.
    pub fn counters(&mut self, destination: &str) -> &mut Counters {
        if !self.destinations.contains_key(destination)
            && self.destinations.len() >= MAX_REPORTED_DESTINATIONS
        {
            return &mut self.other;
        }
        self.destinations.entry(destination.to_owned()).or_default()
    }

    /// One line per destination for the evidence, `other` last when it counted anything.
    pub fn describe(&self) -> Vec<String> {
        let mut lines: Vec<String> = self
            .destinations
            .iter()
            .map(|(destination, counters)| format!("egress {destination}: {}", counters.describe()))
            .collect();
        if self.other != Counters::default() {
            lines.push(format!(
                "egress other destinations: {}",
                self.other.describe()
            ));
        }
        lines
    }
}

/// What a subject may reach, as one snapshot: the listener decides every connection on it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allowlist {
    /// The program the subject runs; a refusal names it when it declares no host (#861).
    pub program: String,
    pub entries: Vec<AllowEntry>,
}

/// One destination the subject may reach, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowEntry {
    pub target: Target,
    pub port: u16,
    pub source: AllowSource,
}

/// What an entry names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Target {
    /// A lower-case DNS name, or `*.` and a domain for every name under it (not the domain
    /// itself).
    Name(String),
    /// An address, granted as such: the one way to reach a loopback, private or link-local
    /// address, or to `CONNECT` to an IP literal.
    Address(IpAddr),
}

/// Where an entry came from (#861): the evidence and the counters name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AllowSource {
    /// A provider adapter's declared hosts (#830).
    Adapter { provider: String },
    /// The registry of an ecosystem the checkout uses (#859).
    Ecosystem { ecosystem: String },
    /// The owner's "allow always" of a host for the subject's project (#674): a revocable
    /// grant, named by its id.
    Grant { grant: String },
    /// The owner's "allow once" of a network request (#674): the host stays allowed for the
    /// rest of the session that asked.
    Request { request: i64 },
}

impl fmt::Display for AllowSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AllowSource::Adapter { provider } => write!(formatter, "the {provider} adapter"),
            AllowSource::Ecosystem { ecosystem } => {
                write!(formatter, "the {ecosystem} registry")
            }
            AllowSource::Grant { grant } => write!(formatter, "grant {grant}"),
            AllowSource::Request { request } => {
                write!(
                    formatter,
                    "network request {request}, allowed for this session"
                )
            }
        }
    }
}

impl fmt::Display for AllowEntry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.target {
            Target::Name(name) => write!(formatter, "{name}:{}", self.port),
            Target::Address(IpAddr::V4(address)) => write!(formatter, "{address}:{}", self.port),
            Target::Address(IpAddr::V6(address)) => {
                write!(formatter, "[{address}]:{}", self.port)
            }
        }
    }
}

/// A destination the allowlist lets through, and what the listener still checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Allowed<'a> {
    pub entry: &'a AllowEntry,
    /// The `CONNECT` host was a name: the listener resolves it and passes every address
    /// through [`Allowlist::check_addresses`].
    pub is_name: bool,
    /// The TLS ClientHello must name the `CONNECT` host: a name on port 443.
    pub expects_server_name: bool,
}

/// Why a connection does not leave the sandbox, by policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The `CONNECT` host is no DNS name or IP address.
    Malformed { host: String },
    /// The program declares no host, and nothing else allows one (#861).
    NoHostsDeclared {
        program: String,
        destination: String,
    },
    /// No entry names this host and port.
    NotAllowed { destination: String },
    /// An IP literal no entry grants.
    AddressLiteral { destination: String },
    /// The name resolves to an address outside the public internet that no entry grants.
    ForbiddenAddress {
        destination: String,
        address: IpAddr,
        class: &'static str,
    },
}

impl Refusal {
    /// Whether the owner can lift the refusal by allowing the name (#674): no entry names
    /// it. Addresses and malformed hosts are refused without asking.
    pub fn is_askable(&self) -> bool {
        matches!(
            self,
            Refusal::NotAllowed { .. } | Refusal::NoHostsDeclared { .. }
        )
    }

    /// Whether the refusal is about an address rather than a name: the proxy answers
    /// `destination_ip_prohibited` for it.
    pub fn is_about_an_address(&self) -> bool {
        matches!(
            self,
            Refusal::AddressLiteral { .. } | Refusal::ForbiddenAddress { .. }
        )
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Malformed { host } => {
                write!(formatter, "{host:?} is not a host name or IP address")
            }
            Refusal::NoHostsDeclared {
                program,
                destination,
            } => write!(
                formatter,
                "{destination} is not allowed: `{program}` declares no hosts"
            ),
            Refusal::NotAllowed { destination } => write!(
                formatter,
                "{destination} is not allowed: no entry of the allowlist names it"
            ),
            Refusal::AddressLiteral { destination } => write!(
                formatter,
                "{destination} is an IP address; only an address granted as such is reached"
            ),
            Refusal::ForbiddenAddress {
                destination,
                address,
                class,
            } => write!(
                formatter,
                "{destination} resolves to {address}, a {class} address, which no entry grants"
            ),
        }
    }
}

impl Allowlist {
    /// This allowlist with the entries the owner allowed added after its own.
    pub fn with_decisions(&self, decisions: &EgressDecisions) -> Allowlist {
        Allowlist {
            program: self.program.clone(),
            entries: self
                .entries
                .iter()
                .chain(&decisions.allowed)
                .cloned()
                .collect(),
        }
    }

    /// The entry that lets `host` (a `CONNECT` target's host, brackets removed) on `port`
    /// through, or why none does.
    pub fn decide(&self, host: &str, port: u16) -> Result<Allowed<'_>, Refusal> {
        if let Ok(address) = host.parse::<IpAddr>() {
            let destination = destination(host, port);
            return self
                .granted_address(address, port)
                .map(|entry| Allowed {
                    entry,
                    is_name: false,
                    expects_server_name: false,
                })
                .ok_or(Refusal::AddressLiteral { destination });
        }
        let name = normalized_name(host).ok_or_else(|| Refusal::Malformed {
            host: host.to_owned(),
        })?;
        let destination = destination(&name, port);
        let entry = self
            .entries
            .iter()
            .find(|entry| {
                entry.port == port
                    && matches!(&entry.target, Target::Name(pattern) if name_matches(pattern, &name))
            })
            .ok_or_else(|| {
                if self.entries.is_empty() {
                    Refusal::NoHostsDeclared {
                        program: self.program.clone(),
                        destination: destination.clone(),
                    }
                } else {
                    Refusal::NotAllowed {
                        destination: destination.clone(),
                    }
                }
            })?;
        Ok(Allowed {
            entry,
            is_name: true,
            expects_server_name: port == TLS_PORT,
        })
    }

    /// Refuses the connection when any address `host` resolved to is outside the public
    /// internet and no entry grants it on `port`: one bad answer refuses them all.
    pub fn check_addresses(
        &self,
        host: &str,
        port: u16,
        addresses: &[IpAddr],
    ) -> Result<(), Refusal> {
        for &address in addresses {
            if let Some(class) = non_public_class(address)
                && self.granted_address(address, port).is_none()
            {
                return Err(Refusal::ForbiddenAddress {
                    destination: destination(host, port),
                    address,
                    class,
                });
            }
        }
        Ok(())
    }

    fn granted_address(&self, address: IpAddr, port: u16) -> Option<&AllowEntry> {
        let address = canonical(address);
        self.entries.iter().find(|entry| {
            entry.port == port
                && matches!(entry.target, Target::Address(granted) if canonical(granted) == address)
        })
    }
}

/// `host:port`, an IPv6 address in brackets: how counters and refusals name a destination.
pub fn destination(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// `host` in lower case without one trailing dot, when it is a DNS name.
pub fn normalized_name(host: &str) -> Option<String> {
    let name = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    let is_name = !name.is_empty()
        && name.len() <= 253
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        });
    is_name.then_some(name)
}

/// `pattern` is the name itself, or `*.domain` and `name` lies under the domain.
fn name_matches(pattern: &str, name: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(domain) => name
            .strip_suffix(domain)
            .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.')),
        None => pattern == name,
    }
}

/// An IPv4-mapped IPv6 address as the IPv4 address it is.
fn canonical(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        IpAddr::V4(_) => address,
    }
}

/// What kind of address outside the public internet `address` is; None for a public one.
pub fn non_public_class(address: IpAddr) -> Option<&'static str> {
    match canonical(address) {
        IpAddr::V4(v4) => v4_class(v4),
        IpAddr::V6(v6) => v6_class(v6),
    }
}

fn v4_class(address: Ipv4Addr) -> Option<&'static str> {
    let [first, second, third, _] = address.octets();
    let class = match (first, second, third) {
        (0, _, _) => "this-network",
        (10, _, _) | (172, 16..=31, _) | (192, 168, _) => "private",
        (100, 64..=127, _) => "shared (carrier-grade NAT)",
        (127, _, _) => "loopback",
        (169, 254, _) => "link-local (cloud metadata)",
        (192, 0, 0) => "IETF protocol",
        (192, 0, 2) | (198, 51, 100) | (203, 0, 113) => "documentation",
        (198, 18..=19, _) => "benchmarking",
        (224..=239, _, _) => "multicast",
        (240..=255, _, _) => "reserved",
        _ => return None,
    };
    Some(class)
}

fn v6_class(address: Ipv6Addr) -> Option<&'static str> {
    let segments = address.segments();
    let embedded_v4 = |high: u16, low: u16| {
        let [a, b] = high.to_be_bytes();
        let [c, d] = low.to_be_bytes();
        Ipv4Addr::new(a, b, c, d)
    };
    if address.is_unspecified() {
        return Some("unspecified");
    }
    if address.is_loopback() {
        return Some("loopback");
    }
    match segments {
        // IPv4-compatible (deprecated): routed as the IPv4 address inside.
        [0, 0, 0, 0, 0, 0, _, _] => Some("IPv4-compatible"),
        // NAT64: the IPv4 address inside decides.
        [0x64, 0xff9b, 0, 0, 0, 0, high, low] => v4_class(embedded_v4(high, low)),
        [0x64, 0xff9b, 1, ..] => Some("local NAT64"),
        [0x100, 0, 0, 0, ..] => Some("discard"),
        [0x2001, 0xdb8, ..] => Some("documentation"),
        // Teredo and the other IETF protocol assignments.
        [0x2001, 0..=0x1ff, ..] => Some("IETF protocol"),
        // 6to4: the IPv4 address inside decides.
        [0x2002, high, low, ..] => v4_class(embedded_v4(high, low)),
        [first, ..] if first & 0xfe00 == 0xfc00 => Some("unique local (private)"),
        [first, ..] if first & 0xffc0 == 0xfe80 => Some("link-local"),
        [first, ..] if first & 0xffc0 == 0xfec0 => Some("site-local"),
        [first, ..] if first & 0xff00 == 0xff00 => Some("multicast"),
        _ => None,
    }
}
