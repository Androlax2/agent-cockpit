//! The instruction manifest: every packet an agent is given, with its source, scope, version,
//! content hash, trust decision, precedence and what it supersedes (report 08). A hash is
//! identity, not trust: these packets are trusted because the product ships them.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bundle::{VERSION, documents};

/// Where a packet applies. Only the product's own bundle exists so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Product,
}

/// Why a packet is believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trust {
    /// Shipped with the installed release.
    Product,
}

/// How a packet ranks against others on the same subject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Precedence {
    /// Reserved: no user or project guidance may shadow it.
    ProductInvariant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Relative to the bundle directory.
    pub path: String,
    pub source: String,
    pub scope: Scope,
    pub version: String,
    /// Of the file as written, stamp included.
    pub sha256: String,
    pub trust: Trust,
    pub precedence: Precedence,
    /// Paths of the packets this one shadows; a product invariant shadows nothing.
    pub supersedes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`manifest_id`] of the entries: the instruction-manifest id the launch brief names.
    pub id: String,
    pub version: String,
    /// The manifest id of another release's bundle this one replaced on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    pub entries: Vec<ManifestEntry>,
}

/// The manifest of the bundle in this binary.
pub fn manifest() -> Manifest {
    let entries: Vec<ManifestEntry> = documents()
        .iter()
        .map(|document| ManifestEntry {
            path: document.path.to_owned(),
            source: "cockpit".to_owned(),
            scope: Scope::Product,
            version: VERSION.to_owned(),
            sha256: hex(&Sha256::digest(document.file_text().as_bytes())),
            trust: Trust::Product,
            precedence: Precedence::ProductInvariant,
            supersedes: Vec::new(),
        })
        .collect();
    Manifest {
        id: manifest_id(&entries),
        version: VERSION.to_owned(),
        supersedes: None,
        entries,
    }
}

/// `m-` and 16 hex digits of the SHA-256 over each entry's path and hash: any changed packet
/// changes it, so a brief names exactly the instructions it was written against.
pub fn manifest_id(entries: &[ManifestEntry]) -> String {
    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update(entry.path.as_bytes());
        hasher.update([0]);
        hasher.update(entry.sha256.as_bytes());
        hasher.update(b"\n");
    }
    let digest = hex(&hasher.finalize());
    format!("m-{}", &digest[..16])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
