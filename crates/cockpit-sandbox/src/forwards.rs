//! Host-service forwards (#860, #880): a declared service the launcher serves inside the
//! sandbox at its `listen` endpoint and relays, byte for byte, to its `target` on the host,
//! once the owner granted the service's whole record. The record is what a grant is bound
//! to: a changed target, listen or identity check is another record, which the grant does
//! not cover.
//!
//! A forward is never the session proxy's: the proxy keeps refusing loopback (#831), and a
//! forward parses nothing it relays.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::egress::{AllowSource, PROXY_ADDRESS};

/// The most forwards one launch serves: each is a listener handed out of the sandbox.
pub const MAX_FORWARDS: usize = 32;

/// One end of a forward: a loopback port, or a named Unix socket.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Endpoint {
    /// `127.0.0.1:port`: inside the sandbox's own loopback for a `listen`, the host's for a
    /// `target`.
    Port { port: u16 },
    /// A named Unix socket. As a `listen`, the launcher creates it in a folder of its own,
    /// bound read-only at the path's folder, so the program cannot replace it.
    Socket { path: PathBuf },
}

impl fmt::Display for Endpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Endpoint::Port { port } => write!(formatter, "127.0.0.1:{port}"),
            Endpoint::Socket { path } => write!(formatter, "unix:{}", path.display()),
        }
    }
}

/// One declared service the launcher serves inside the sandbox. It grants nothing: the
/// launcher relays a connection only while the owner's decisions allow [`Forward::record`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forward {
    /// The service's name, as its declaration gives it.
    pub service: String,
    /// Where the program connects, inside the sandbox.
    pub listen: Endpoint,
    /// What the launcher connects to, on the host.
    pub target: Endpoint,
}

impl Forward {
    /// The record the owner's grant is bound to: every field that decides what the program
    /// reaches. Two forwards with one record reach the same thing.
    pub fn record(&self) -> String {
        format!(
            "service {} listen {} target {}",
            self.service, self.listen, self.target
        )
    }

    /// The listen address of a port forward, as stage 2 binds it.
    pub fn listen_port(&self) -> Option<u16> {
        match self.listen {
            Endpoint::Port { port } => Some(port),
            Endpoint::Socket { .. } => None,
        }
    }
}

/// A lower-case ASCII letter, then letters, digits and `-`, at most 64 bytes: a service name
/// the owner reads in a request and the counters key on. Like a socket path the spec
/// accepts, it holds no space, so a record reads one way only.
pub fn is_service_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    name.len() <= 64
        && bytes.next().is_some_and(|first| first.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// The port of [`PROXY_ADDRESS`], which no forward may listen on.
pub fn proxy_port() -> u16 {
    PROXY_ADDRESS
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse().ok())
        .expect("the proxy address ends with its port")
}

/// The owner's decisions on a subject's service requests, beside its host decisions in
/// [`crate::EgressDecisions`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ServiceDecisions {
    /// The records the owner allowed: for this session or always for its project.
    pub allowed: Vec<AllowedService>,
    /// The records the owner denied this subject.
    pub denied: Vec<DecidedService>,
    /// The records asked of the owner and not yet decided.
    pub asking: Vec<DecidedService>,
}

/// A record the owner allowed, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowedService {
    pub service: String,
    pub record: String,
    pub source: AllowSource,
}

/// A record the owner denied, or is asked about, and its request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecidedService {
    pub service: String,
    pub record: String,
    pub request: i64,
}

impl ServiceDecisions {
    /// What allows `forward`'s record now, if anything does.
    pub fn allowing(&self, forward: &Forward) -> Option<&AllowedService> {
        self.allowing_record(&forward.service, &forward.record())
    }

    /// What allows `service`'s `record` now, if anything does: the one match every check of
    /// a service's grant goes through.
    pub fn allowing_record(&self, service: &str, record: &str) -> Option<&AllowedService> {
        self.allowed
            .iter()
            .find(|allowed| allowed.service == service && allowed.record == record)
    }

    /// The request whose denial covers `forward`'s record.
    pub fn denial(&self, forward: &Forward) -> Option<i64> {
        find(&self.denied, &forward.service, &forward.record())
    }

    /// The request waiting on the owner for `forward`'s record.
    pub fn asking(&self, forward: &Forward) -> Option<i64> {
        find(&self.asking, &forward.service, &forward.record())
    }

    /// Whether the owner decided `forward`'s record: allowed or denied.
    pub fn covers(&self, forward: &Forward) -> bool {
        self.covers_record(&AskedService::of(forward))
    }

    /// Whether the owner decided the record `asked` names.
    pub fn covers_record(&self, asked: &AskedService) -> bool {
        self.allowing_record(&asked.service, &asked.record)
            .is_some()
            || find(&self.denied, &asked.service, &asked.record).is_some()
    }
}

fn find(decided: &[DecidedService], service: &str, record: &str) -> Option<i64> {
    decided
        .iter()
        .find(|decided| decided.service == service && decided.record == record)
        .map(|decided| decided.request)
}

/// A service record a launcher refused for want of a decision, as it lists it in
/// [`crate::REQUESTS_FILE`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AskedService {
    pub service: String,
    pub record: String,
}

impl AskedService {
    pub fn of(forward: &Forward) -> AskedService {
        AskedService {
            service: forward.service.clone(),
            record: forward.record(),
        }
    }
}
