//! The instruction bundle and launch brief on the wire (#49). The daemon serves the bundle of
//! the release it runs, so a reader always gets the instructions matching it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionTopicView {
    pub name: String,
    /// When to read it.
    pub summary: String,
}

/// How much context Cockpit adds, in bytes: the brief and the skill's catalog entry are
/// loaded into every conversation; the manual is read on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBudgetView {
    pub brief_bytes: u64,
    pub catalog_bytes: u64,
    pub on_demand_bytes: u64,
}
