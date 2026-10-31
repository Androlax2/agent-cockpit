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
    /// A reason this version does not know, kept as sent.
    #[serde(untagged)]
    Unknown(String),
}

/// Agrees on the highest version both sides speak and the capabilities both offer, in the
/// daemon's order.
pub fn negotiate(hello: &Hello, daemon_capabilities: &[Capability]) -> HandshakeReply {
    let shared_version = SUPPORTED_VERSIONS
        .iter()
        .rev()
        .find(|version| hello.protocol_versions.contains(version));
    let Some(&protocol_version) = shared_version else {
        return HandshakeReply::Refused(version_mismatch(hello));
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

fn version_mismatch(hello: &Hello) -> Refusal {
    Refusal {
        reason: RefusalReason::VersionMismatch,
        message: format!(
            "the client speaks protocol versions [{}]; this daemon speaks [{}]",
            joined(&hello.protocol_versions),
            joined(&SUPPORTED_VERSIONS)
        ),
        supported_versions: SUPPORTED_VERSIONS.to_vec(),
    }
}

fn joined(versions: &[ProtocolVersion]) -> String {
    versions
        .iter()
        .map(|version| version.0.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}
