//! The readiness report `cockpit doctor` prints (#50, report 09).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadinessStatusView {
    Available,
    NeedsAuthorization,
    MissingPrerequisite,
    Unsupported,
    Unknown,
}

impl ReadinessStatusView {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::NeedsAuthorization => "needs-authorization",
            Self::MissingPrerequisite => "missing-prerequisite",
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
        }
    }

    /// Work that needs it cannot go ahead: `cockpit doctor` exits 1.
    pub fn blocks(self) -> bool {
        matches!(self, Self::MissingPrerequisite | Self::Unsupported)
    }
}

/// One requirement: what was found and, unless it is available, what makes it so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequirementView {
    pub name: String,
    pub status: ReadinessStatusView,
    pub detail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}
