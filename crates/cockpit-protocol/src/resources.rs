//! Wire views of resource admission (#42). Workloads and controls travel as their names, so
//! a newer daemon's names reach an older client unchanged.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProfileView {
    Normal,
    /// Heavy local jobs get a smaller share and ceilings; agent turns, deliveries and
    /// approvals are never held by it.
    Foreground,
    #[serde(other)]
    Unknown,
}

impl ProfileView {
    /// The wire name; `unknown` for a value this version cannot read.
    pub fn as_str(self) -> &'static str {
        match self {
            ProfileView::Normal => "normal",
            ProfileView::Foreground => "foreground",
            ProfileView::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DemandView {
    /// Thousandths of one CPU.
    pub cpu_millis: u64,
    pub memory_mib: u64,
}

/// The budget, the owner's settings, what this host enforces, and every reservation still
/// charged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceStatusView {
    /// `linux`, `wsl` or `macos`.
    pub platform: String,
    /// What holds new executions: `systemd-scope` or `process-group`.
    pub containment: String,
    pub profile: ProfileView,
    pub queue_paused: bool,
    pub budget: DemandView,
    pub in_use: DemandView,
    /// Under the foreground profile, heavy jobs together get at most this share of the budget.
    pub foreground_heavy_percent: u64,
    /// Every control, for new executions.
    pub controls: Vec<ControlSupportView>,
    pub reservations: Vec<ReservationView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlSupportView {
    pub control: String,
    pub supported: bool,
    /// The mechanism when supported, the missing piece when not.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReservationView {
    pub id: i64,
    pub label: String,
    pub workload: String,
    /// `reserved` (not started), `running`, or `held` (an agent turn scheduling placed, #386).
    pub state: String,
    pub execution: Option<i64>,
    /// What the budget is charged.
    pub reserved: DemandView,
    pub required: Vec<String>,
    /// Pause at checkpoint was requested.
    pub checkpoint_requested: bool,
    /// Requested, supported and effective for every control.
    pub limits: Vec<LimitView>,
    /// What the execution uses now, where the host measures it.
    pub observed: ObservedView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LimitView {
    pub control: String,
    /// What Cockpit asks the host for in the current profile; `None` when nothing.
    pub requested: Option<String>,
    pub supported: bool,
    pub effective: EffectiveView,
}

/// What the kernel shows is in force, never what was asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EffectiveView {
    Value { value: String },
    Unknown { why: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ObservedView {
    pub memory_bytes: Option<u64>,
    pub tasks: Option<u64>,
}

/// Whether work would be admitted now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum VerdictView {
    Fits,
    /// `reason` names the cause: the paused queue, a control the host cannot enforce, or
    /// the capacity it does not fit.
    Held {
        reason: String,
    },
}
