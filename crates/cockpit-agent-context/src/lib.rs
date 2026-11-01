#![doc = include_str!("../README.md")]

mod brief;
mod bundle;
mod manifest;
mod write;

pub use brief::{
    BRIEF_TARGET_BYTES, ContextBudget, Grants, LaunchFacts, WorkflowFacts, brief, context_budget,
};
pub use bundle::{CatalogEntry, Document, VERSION, catalog, document, documents, manual};
pub use manifest::{Manifest, ManifestEntry, Precedence, Scope, Trust, manifest, manifest_id};
pub use write::{MANIFEST_FILE, WriteError, write_bundle};
