//! What admission reasons about: workloads, their reserved demand, the limits asked for them,
//! the profile, and why work is held.

use std::fmt;
use std::time::Duration;

use cockpit_processes::ExecutionId;

/// One reservation's row id; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ReservationId(pub i64);

impl fmt::Display for ReservationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "reservation {}", self.0)
    }
}

/// What kind of work asks for capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workload {
    AgentTurn,
    Delivery,
    Approval,
    Build,
    Test,
    Container,
}

impl Workload {
    pub const ALL: [Workload; 6] = [
        Workload::AgentTurn,
        Workload::Delivery,
        Workload::Approval,
        Workload::Build,
        Workload::Test,
        Workload::Container,
    ];

    /// Heavy local jobs. The foreground profile limits these and never the others: agent
    /// turns, deliveries and approvals keep going while Theo games (decided 2026-10-31).
    pub fn is_heavy(self) -> bool {
        matches!(self, Workload::Build | Workload::Test | Workload::Container)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Workload::AgentTurn => "agent-turn",
            Workload::Delivery => "delivery",
            Workload::Approval => "approval",
            Workload::Build => "build",
            Workload::Test => "test",
            Workload::Container => "container",
        }
    }

    pub fn parse(name: &str) -> Option<Workload> {
        Workload::ALL
            .into_iter()
            .find(|workload| workload.as_str() == name)
    }
}

/// Reserved capacity: what the budget is charged, whatever the work then uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Demand {
    /// Thousandths of one CPU.
    pub cpu_millis: u64,
    pub memory_mib: u64,
}

impl Demand {
    fn get(self, resource: Resource) -> u64 {
        match resource {
            Resource::Cpu => self.cpu_millis,
            Resource::Memory => self.memory_mib,
        }
    }

    fn scaled(self, percent: u64) -> Demand {
        Demand {
            cpu_millis: self.cpu_millis * percent / 100,
            memory_mib: self.memory_mib * percent / 100,
        }
    }

    pub(crate) fn plus(self, other: Demand) -> Demand {
        Demand {
            cpu_millis: self.cpu_millis + other.cpu_millis,
            memory_mib: self.memory_mib + other.memory_mib,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Cpu,
    Memory,
}

impl Resource {
    pub fn unit(self) -> &'static str {
        match self {
            Resource::Cpu => "millicores",
            Resource::Memory => "MiB",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Resource::Cpu => "cpu",
            Resource::Memory => "memory",
        }
    }
}

/// A control a backend may or may not enforce. Weight and quota are separate controls:
/// weight is a priority under contention, quota a ceiling (report 10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Control {
    CpuWeight,
    CpuQuota,
    MemoryMax,
    TasksMax,
    IdleScheduling,
    Nice,
    Freeze,
}

impl Control {
    pub const ALL: [Control; 7] = [
        Control::CpuWeight,
        Control::CpuQuota,
        Control::MemoryMax,
        Control::TasksMax,
        Control::IdleScheduling,
        Control::Nice,
        Control::Freeze,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Control::CpuWeight => "cpu-weight",
            Control::CpuQuota => "cpu-quota",
            Control::MemoryMax => "memory-max",
            Control::TasksMax => "tasks-max",
            Control::IdleScheduling => "idle-scheduling",
            Control::Nice => "nice",
            Control::Freeze => "freeze",
        }
    }

    pub fn parse(name: &str) -> Option<Control> {
        Control::ALL
            .into_iter()
            .find(|control| control.as_str() == name)
    }
}

/// The limits asked for one execution; `None` and `false` ask for nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Limits {
    /// Priority under contention, 1 to 10000 (systemd's `CPUWeight=`; 100 is the default).
    pub cpu_weight: Option<u64>,
    /// Ceiling in percent of one CPU (`CPUQuota=`; 200 is two CPUs).
    pub cpu_quota_percent: Option<u64>,
    pub memory_max_mib: Option<u64>,
    pub tasks_max: Option<u64>,
    /// Run under `SCHED_IDLE`: only when a CPU would otherwise idle.
    pub idle_scheduling: bool,
    /// Niceness, 0 to 19.
    pub nice: Option<u64>,
}

pub(crate) const MAX_CPU_WEIGHT: u64 = 10_000;
pub(crate) const MAX_NICE: u64 = 19;

impl Limits {
    /// The value asked for `control`, as the report prints it; `None` when not asked.
    pub fn requested(&self, control: Control) -> Option<String> {
        match control {
            Control::CpuWeight => self.cpu_weight.map(|weight| weight.to_string()),
            Control::CpuQuota => self.cpu_quota_percent.map(|quota| format!("{quota}%")),
            Control::MemoryMax => self.memory_max_mib.map(|mib| format!("{mib}M")),
            Control::TasksMax => self.tasks_max.map(|tasks| tasks.to_string()),
            Control::IdleScheduling => self.idle_scheduling.then(|| "idle".to_owned()),
            Control::Nice => self.nice.map(|nice| nice.to_string()),
            Control::Freeze => None,
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self
            .cpu_weight
            .is_some_and(|weight| !(1..=MAX_CPU_WEIGHT).contains(&weight))
        {
            return Err(format!("cpu weight must be 1 to {MAX_CPU_WEIGHT}"));
        }
        if self.cpu_quota_percent == Some(0) {
            return Err("a cpu quota of 0% would never run".to_owned());
        }
        if self.memory_max_mib == Some(0) || self.tasks_max == Some(0) {
            return Err("a memory or tasks ceiling of 0 would never run".to_owned());
        }
        if self.nice.is_some_and(|nice| nice > MAX_NICE) {
            return Err(format!("nice must be 0 to {MAX_NICE}"));
        }
        Ok(())
    }
}

/// What a caller asks admission for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionRequest {
    /// Who the work is for, as reports show it (a task, a session, a command).
    pub label: String,
    pub workload: Workload,
    pub demand: Demand,
    pub limits: Limits,
    /// Controls the work must not start without: one the backend cannot enforce holds it.
    pub required: Vec<Control>,
}

/// The owner's profile. Agents cannot award themselves foreground priority: only the owner's
/// socket changes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Normal,
    /// Theo plays or works in the foreground: heavy jobs get a smaller share and ceilings.
    Foreground,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Normal => "normal",
            Profile::Foreground => "foreground",
        }
    }

    pub fn parse(name: &str) -> Option<Profile> {
        [Profile::Normal, Profile::Foreground]
            .into_iter()
            .find(|profile| profile.as_str() == name)
    }
}

/// The admission settings the owner changes; durable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub profile: Profile,
    /// Pause queue: start nothing new until resumed.
    pub queue_paused: bool,
}

/// The budget and its rules. The daemon builds it from the host's capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Everything Cockpit runs shares this: agents, builds, tests, containers.
    pub budget: Demand,
    /// Under the foreground profile, heavy jobs together get at most this share of the budget.
    pub foreground_heavy_percent: u64,
    /// A reservation whose work never started is released after this long.
    pub unstarted_expiry: Duration,
}

impl Policy {
    pub const DEFAULT_FOREGROUND_HEAVY_PERCENT: u64 = 25;
    pub const DEFAULT_UNSTARTED_EXPIRY: Duration = Duration::from_secs(10 * 60);

    pub fn for_budget(budget: Demand) -> Policy {
        Policy {
            budget,
            foreground_heavy_percent: Policy::DEFAULT_FOREGROUND_HEAVY_PERCENT,
            unstarted_expiry: Policy::DEFAULT_UNSTARTED_EXPIRY,
        }
    }

    /// The limits to enforce on `workload` in `profile`. The foreground profile adds
    /// ceilings to heavy jobs: lowest weight, a quota no larger than the reservation, idle
    /// scheduling and nice 19. A stricter request is kept.
    pub fn enforced_limits(
        &self,
        workload: Workload,
        demand: Demand,
        requested: Limits,
        profile: Profile,
    ) -> Limits {
        if profile == Profile::Normal || !workload.is_heavy() {
            return requested;
        }
        let reservation_quota = demand.cpu_millis.div_ceil(10).max(1);
        Limits {
            cpu_weight: Some(1),
            cpu_quota_percent: Some(
                requested
                    .cpu_quota_percent
                    .map_or(reservation_quota, |quota| quota.min(reservation_quota)),
            ),
            memory_max_mib: requested.memory_max_mib,
            tasks_max: requested.tasks_max,
            idle_scheduling: true,
            nice: Some(MAX_NICE),
        }
    }
}

/// What holds work back. Every hold names its cause so a waiting item can be explained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hold {
    /// The owner paused the queue: nothing new starts.
    QueuePaused,
    /// A required control cannot be enforced here; `why` is the backend's evidence.
    ControlUnsupported { control: Control, why: String },
    /// The work can never fit `capacity` as it stands, even with nothing else running.
    Infeasible {
        resource: Resource,
        demand: u64,
        capacity: u64,
        limited_by: Capacity,
    },
    /// The work fits once `demand - available` is released.
    NoRoom {
        resource: Resource,
        demand: u64,
        available: u64,
        limited_by: Capacity,
    },
}

/// Which capacity a hold is measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capacity {
    Budget,
    /// The heavy-job share the foreground profile leaves.
    ForegroundShare,
}

impl fmt::Display for Hold {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Hold::QueuePaused => write!(formatter, "the queue is paused: nothing new starts"),
            Hold::ControlUnsupported { control, why } => write!(
                formatter,
                "{} is required but cannot be enforced here: {why}",
                control.as_str()
            ),
            Hold::Infeasible {
                resource,
                demand,
                capacity,
                limited_by,
            } => write!(
                formatter,
                "needs {demand} {unit} of {resource}; {} is {capacity}, so it can never start",
                limited_by.describe(),
                unit = resource.unit(),
                resource = resource.as_str(),
            ),
            Hold::NoRoom {
                resource,
                demand,
                available,
                limited_by,
            } => write!(
                formatter,
                "needs {demand} {unit} of {resource}; {available} are free in {}",
                limited_by.describe(),
                unit = resource.unit(),
                resource = resource.as_str(),
            ),
        }
    }
}

impl Capacity {
    fn describe(self) -> &'static str {
        match self {
            Capacity::Budget => "the shared budget",
            Capacity::ForegroundShare => "the foreground profile's heavy-job share",
        }
    }
}

/// Whether a request fits now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Fits,
    Held(Hold),
}

/// What [`crate::admit`] did with a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    Admitted(ReservationId),
    Held(Hold),
}

/// Capacity charged now: every reservation not released, and the heavy ones among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InUse {
    pub all: Demand,
    pub heavy: Demand,
}

/// The rules of admission, in the order a hold is reported: pause first, then what can never
/// be enforced, then what can never fit, then what does not fit yet.
pub(crate) fn decide(
    request: &AdmissionRequest,
    settings: Settings,
    in_use: InUse,
    policy: &Policy,
    support: &[ControlSupport],
) -> Verdict {
    if settings.queue_paused {
        return Verdict::Held(Hold::QueuePaused);
    }
    for control in &request.required {
        if let Some(Availability::Unsupported { why }) = support
            .iter()
            .find(|support| support.control == *control)
            .map(|support| &support.availability)
        {
            return Verdict::Held(Hold::ControlUnsupported {
                control: *control,
                why: why.clone(),
            });
        }
    }
    let mut limits = vec![(Capacity::Budget, policy.budget, in_use.all)];
    if settings.profile == Profile::Foreground && request.workload.is_heavy() {
        limits.push((
            Capacity::ForegroundShare,
            policy.budget.scaled(policy.foreground_heavy_percent),
            in_use.heavy,
        ));
    }
    for (limited_by, capacity, used) in limits {
        for resource in [Resource::Cpu, Resource::Memory] {
            let demand = request.demand.get(resource);
            let capacity = capacity.get(resource);
            if demand > capacity {
                return Verdict::Held(Hold::Infeasible {
                    resource,
                    demand,
                    capacity,
                    limited_by,
                });
            }
            let available = capacity.saturating_sub(used.get(resource));
            if demand > available {
                return Verdict::Held(Hold::NoRoom {
                    resource,
                    demand,
                    available,
                    limited_by,
                });
            }
        }
    }
    Verdict::Fits
}

/// Whether the backend enforces `control` for new executions, with its evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlSupport {
    pub control: Control,
    pub availability: Availability,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// `how` names the mechanism, so the claim can be checked.
    Supported { how: String },
    /// `why` names the missing piece.
    Unsupported { why: String },
}

/// Where a reservation is in its life.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReservationState {
    /// Admitted, not started; released at `expires_at` (seconds since the epoch).
    Reserved { expires_at: i64 },
    /// Its execution started; released only once that execution is observed ended.
    Running { execution: ExecutionId },
    Released { reason: ReleaseReason },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseReason {
    /// Never started before its expiry.
    Expired,
    /// Its caller gave it back before starting.
    Withdrawn,
    /// Its execution was observed ended.
    Ended,
}

impl ReleaseReason {
    pub fn as_str(self) -> &'static str {
        match self {
            ReleaseReason::Expired => "expired",
            ReleaseReason::Withdrawn => "withdrawn",
            ReleaseReason::Ended => "ended",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<ReleaseReason> {
        [
            ReleaseReason::Expired,
            ReleaseReason::Withdrawn,
            ReleaseReason::Ended,
        ]
        .into_iter()
        .find(|reason| reason.as_str() == name)
    }
}

/// A reservation as the state database records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub id: ReservationId,
    pub label: String,
    pub workload: Workload,
    pub demand: Demand,
    /// The limits the caller asked for, before any profile ceiling.
    pub limits: Limits,
    pub required: Vec<Control>,
    pub state: ReservationState,
    /// Pause at checkpoint: the owner asked the work to stop at its next supported boundary.
    pub checkpoint_requested: bool,
}

/// What [`crate::sweep`] released.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Swept {
    pub expired: Vec<ReservationId>,
    pub ended: Vec<ReservationId>,
}
