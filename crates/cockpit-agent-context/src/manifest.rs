//! The instruction manifest: every packet an agent is given, with its source, scope, version,
//! content hash, trust decision, precedence and what it supersedes (report 08). A hash is
//! identity, not trust: the product's packets are trusted because the product ships them, the
//! owner's user guidance because it is the owner's, and a project's guidance only when the
//! owner trusts the project (#420, [`crate::guidance`]).

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::bundle::{VERSION, documents};

/// Where a packet applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Product,
    /// The owner's own guidance file, such as `~/.claude/CLAUDE.md`.
    User,
    /// A file of the session's project: `CLAUDE.md`, `AGENTS.md`, its rules.
    Project,
}

/// Why a packet is believed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trust {
    /// Shipped with the installed release.
    Product,
    /// The owner's own file.
    Owner,
    /// A file of a project the owner trusts (`projects.trusted`, or a clone of one).
    Trusted,
    /// A file of a project without the owner's trust decision, a review clone or a directory
    /// that is no registered project: listed so it is seen, never believed for that.
    Untrusted,
}

impl Trust {
    /// The word the manifest, the brief and [`manifest_id`] use.
    pub fn as_str(self) -> &'static str {
        match self {
            Trust::Product => "product",
            Trust::Owner => "owner",
            Trust::Trusted => "trusted",
            Trust::Untrusted => "untrusted",
        }
    }
}

/// How a packet ranks against others on the same subject; the variants run strongest first,
/// so a smaller one shadows a larger one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Precedence {
    /// Reserved: no user or project guidance may shadow it.
    ProductInvariant,
    /// The owner's policy constrains the guidance of a project.
    User,
    Project,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Relative to the bundle directory for a product packet; the file's own absolute path for
    /// user and project guidance.
    pub path: String,
    pub source: String,
    pub scope: Scope,
    /// The release that ships a product packet; guidance has none, its hash is its version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Of the file as written, stamp included; none for a guidance file that exists and could
    /// not be read, which the manifest still shows instead of leaving it out.
    pub sha256: Option<String>,
    pub trust: Trust,
    pub precedence: Precedence,
    /// Paths of the packets this one shadows: the user's guidance shadows the project's; a
    /// product invariant is shadowed by nothing and, in its own scope, shadows nothing.
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

/// The bundle is fixed per binary, so it is hashed once.
static MANIFEST: LazyLock<Manifest> = LazyLock::new(computed);

/// The manifest of the bundle in this binary.
pub fn manifest() -> Manifest {
    MANIFEST.clone()
}

/// The manifest a session is given: the product's packets, then `guidance` (user, then
/// project; see [`crate::discover_guidance`]). Its id is what that session's brief names.
pub fn session_manifest(guidance: Vec<ManifestEntry>) -> Manifest {
    let mut entries = manifest().entries;
    entries.extend(guidance);
    Manifest {
        id: manifest_id(&entries),
        version: VERSION.to_owned(),
        supersedes: None,
        entries,
    }
}

fn computed() -> Manifest {
    let entries: Vec<ManifestEntry> = documents()
        .iter()
        .map(|document| ManifestEntry {
            path: document.path.to_owned(),
            source: "torlek".to_owned(),
            scope: Scope::Product,
            version: Some(VERSION.to_owned()),
            sha256: Some(hex(&Sha256::digest(document.file_text().as_bytes()))),
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

/// `m-` and 16 hex digits of the SHA-256 over each entry's path, hash and trust: any changed
/// packet, or a project the owner starts or stops trusting, changes it, so a brief names
/// exactly the instructions it was written against.
pub fn manifest_id(entries: &[ManifestEntry]) -> String {
    let mut hasher = Sha256::new();
    for entry in entries {
        hasher.update(entry.path.as_bytes());
        hasher.update([0]);
        hasher.update(entry.sha256.as_deref().unwrap_or("unreadable").as_bytes());
        hasher.update([0]);
        hasher.update(entry.trust.as_str().as_bytes());
        hasher.update(b"\n");
    }
    let digest = hex(&hasher.finalize());
    format!("m-{}", &digest[..16])
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
