use serde::{Deserialize, Serialize};

/// A protocol generation. Additive changes go through capabilities; a new version is only
/// for changes an older peer would misread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProtocolVersion(pub u32);

/// The versions this build of the daemon speaks, oldest first.
pub const SUPPORTED_VERSIONS: [ProtocolVersion; 1] = [ProtocolVersion(1)];

/// An optional feature a peer may offer; granted only when both sides offer it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Subscribing to the change feed.
    ChangeFeed,
    /// A capability this version does not know, kept as sent. Never build one with a known
    /// name: it would read back as the known variant.
    #[serde(untagged)]
    Unknown(String),
}

/// The client's first frame: every version it speaks and the capabilities it wants.
///
/// Its shape must stay readable by every daemon version, since it is read before a
/// version is agreed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_versions: Vec<ProtocolVersion>,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
}

/// The daemon's answer to a [`Hello`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum HandshakeReply {
    Accepted(Welcome),
    Refused(Refusal),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Welcome {
    pub protocol_version: ProtocolVersion,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    pub reason: RefusalReason,
    pub message: String,
    pub supported_versions: Vec<ProtocolVersion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefusalReason {
    /// The client and the daemon share no protocol version.
    VersionMismatch,
    /// The first frame was not a readable [`Hello`].
    MalformedHello,
    /// A reason this version does not know, kept as sent.
    #[serde(untagged)]
    Unknown(String),
}

/// Reads the client's first frame; an unreadable one gets a typed refusal to send back.
/// The daemon passes [`SUPPORTED_VERSIONS`] here and to [`negotiate`].
pub fn decode_hello(frame: &str, daemon_versions: &[ProtocolVersion]) -> Result<Hello, Refusal> {
    serde_json::from_str(frame).map_err(|error| Refusal {
        reason: RefusalReason::MalformedHello,
        message: format!("the first frame is not a valid hello: {error}"),
        supported_versions: daemon_versions.to_vec(),
    })
}

/// Agrees on the highest version both sides speak and the capabilities both offer, in the
/// daemon's order.
pub fn negotiate(
    hello: &Hello,
    daemon_versions: &[ProtocolVersion],
    daemon_capabilities: &[Capability],
) -> HandshakeReply {
    let shared_version = daemon_versions
        .iter()
        .filter(|version| hello.protocol_versions.contains(version))
        .max();
    let Some(&protocol_version) = shared_version else {
        return HandshakeReply::Refused(version_mismatch(hello, daemon_versions));
    };
    let capabilities = daemon_capabilities
        .iter()
        .filter(|capability| hello.capabilities.contains(capability))
        .cloned()
        .collect();
    HandshakeReply::Accepted(Welcome {
        protocol_version,
        capabilities,
    })
}

/// Names only the client's highest version, so a huge list cannot grow the reply; the
/// daemon's versions travel in `supported_versions`.
fn version_mismatch(hello: &Hello, daemon_versions: &[ProtocolVersion]) -> Refusal {
    let client_offer = match hello.protocol_versions.iter().max() {
        Some(highest) => format!("the client speaks versions up to {}", highest.0),
        None => "the client named no version".to_owned(),
    };
    Refusal {
        reason: RefusalReason::VersionMismatch,
        message: format!("no shared protocol version: {client_offer}"),
        supported_versions: daemon_versions.to_vec(),
    }
}
