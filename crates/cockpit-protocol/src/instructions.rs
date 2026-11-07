//! The instruction bundle and launch brief on the wire (#49). The daemon serves the bundle of
//! the release it runs, so a reader always gets the instructions matching it.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionTopicView {
    pub name: String,
    /// When to read it.
    pub summary: String,
}

/// What `cockpit mcp` answers `initialize` with, which every harness that registers the
/// server loads at start; `brief --measure` counts it (#775).
pub const MCP_INSTRUCTIONS: &str = "Torlek's manual for agents. Read torlek://instructions/overview first, \
                                    or call torlek_brief from a Torlek session.";

/// How much context Cockpit adds, in bytes: the brief, the task packet, the MCP instructions
/// and, for a harness that lists the skill, its catalog entry are loaded into every
/// conversation; the manual is read on demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextBudgetView {
    pub brief_bytes: u64,
    /// Zero when the session does no step (#781).
    #[serde(default)]
    pub packet_bytes: u64,
    /// Zero for a harness that does not list the skill (#775).
    pub catalog_bytes: u64,
    #[serde(default)]
    pub mcp_instructions_bytes: u64,
    pub on_demand_bytes: u64,
}
