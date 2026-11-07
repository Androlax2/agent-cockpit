#![doc = include_str!("../README.md")]

mod brief;
mod bundle;
mod manifest;
mod packet;
mod write;

pub use brief::{
    BRIEF_TARGET_BYTES, ContextBudget, GithubTokenFact, Grants, LaunchFacts, WorkflowFacts, brief,
    context_budget,
};
pub use bundle::{CatalogEntry, Document, VERSION, catalog, document, documents, manual};
pub use manifest::{Manifest, ManifestEntry, Precedence, Scope, Trust, manifest, manifest_id};
pub use packet::{TASK_PACKET_CAP_BYTES, task_packet};
pub use write::{MANIFEST_FILE, WriteError, write_bundle};
