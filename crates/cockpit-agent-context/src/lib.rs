#![doc = include_str!("../README.md")]

mod brief;
mod bundle;
mod guidance;
mod manifest;
mod pack;
mod packet;
mod write;

pub use brief::{
    BRIEF_TARGET_BYTES, ContextBudget, Delivery, GithubTokenFact, Grants, LaunchFacts,
    WorkflowFacts, brief, context_budget,
};
pub use bundle::{CatalogEntry, Document, VERSION, catalog, document, documents, manual};
pub use guidance::{GuidanceSearch, discover_guidance};
pub use manifest::{
    Manifest, ManifestEntry, Precedence, Scope, Trust, manifest, manifest_id, session_manifest,
};
pub use pack::{
    InstalledSkill, Licence, ListedSkill, PACK_DIR, Pack, PackChoice, PackError, PackFile,
    PackListing, PackSkill, SkillSearch, SkillState, discover_skills, select_pack, skill_pack,
    write_pack,
};
pub use packet::{TASK_PACKET_CAP_BYTES, task_packet, unreadable_task_packet};
pub use write::{MANIFEST_FILE, WriteError, write_bundle};
