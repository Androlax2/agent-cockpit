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
    /// The optional skill pack's delivered skills, where the harness lists them (#786).
    #[serde(default)]
    pub pack_catalog_bytes: u64,
    pub on_demand_bytes: u64,
}

/// The optional skill pack as the daemon delivered it at its start (#786), beside the skills
/// the owner and the trusted projects already have.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackView {
    pub source: String,
    pub commit: String,
    pub tree: String,
    /// Its SPDX identifier.
    pub licence: String,
    pub enabled: bool,
    pub skills: Vec<PackSkillView>,
    pub installed: Vec<InstalledSkillView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackSkillView {
    pub name: String,
    pub state: PackSkillStateView,
    pub is_starter: bool,
    /// Only the user invokes it; Claude Code keeps its description out of the model's context.
    pub is_user_invoked: bool,
    /// The frontmatter keys with which it grants a harness tools or hooks.
    pub grants: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackSkillStateView {
    Delivered,
    Off,
    /// The installed skill at `by` has its name and wins.
    Shadowed {
        by: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledSkillView {
    pub name: String,
    /// `user` or `project`.
    pub scope: String,
    pub path: String,
    /// Whether the pack holds a skill of the same name.
    pub collides: bool,
}
