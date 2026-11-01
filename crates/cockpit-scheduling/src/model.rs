//! What scheduling reasons about: the sessions that could take work, the waiting work, the
//! placements made, and the explanation of every item that waits.

use std::fmt;
use std::time::Duration;

use cockpit_authorization::SessionId;
use cockpit_provider_api::Capabilities;
use cockpit_resource_admission::{Demand, ReservationId};
use cockpit_tasks::ItemId;
use cockpit_workflows::RunId;

/// The rules of dispatch. The daemon passes the product's values; tests shorten the windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// A session waiting at its prompt gets new work only this long after its turn ended, so
    /// the owner can read the finished turn first (the legacy queue's 5 minutes).
    pub idle_window: Duration,
    /// How often the daemon runs a pass; an explanation names the next one.
    pub interval: Duration,
    /// Waiting work gains one priority level per step waited, up to `aging_max_boost`.
    pub aging_step: Duration,
    pub aging_max_boost: u32,
    /// Work held for room that has waited this long gathers capacity: work ordered after it
    /// is not started with the room it needs, so a large feasible job eventually starts.
    pub gather_after: Duration,
    /// What one agent turn is charged against the shared budget.
    pub turn_demand: Demand,
    /// Added to `turn_demand` in a project no placement has finished in yet: its workload
    /// is unfamiliar, so it starts with headroom (report 10, "startup headroom").
    pub startup_headroom_percent: u64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            idle_window: Duration::from_secs(5 * 60),
            interval: Duration::from_secs(30),
            aging_step: Duration::from_secs(15 * 60),
            aging_max_boost: 4,
            gather_after: Duration::from_secs(30 * 60),
            turn_demand: Demand {
                cpu_millis: 500,
                memory_mib: 1024,
            },
            startup_headroom_percent: 50,
        }
    }
}

/// A team role (report 07). A step that names a role goes only to a session holding it, and
/// a session holding a role takes only its role's steps. Roles come from `cockpit-teams`;
/// until it exists no session and no step holds one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Designer,
    Developer,
    Qa,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Designer => "designer",
            Role::Developer => "developer",
            Role::Qa => "qa",
        }
    }
}

/// Where a session stands, from its lifecycle and its agent's latest report of the current
/// launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Presence {
    /// Not running: launching, stopping, asleep or failed, as `state` names it.
    Inactive {
        state: String,
    },
    /// Running, but its current launch's agent has not reported yet.
    Unreported,
    Working,
    /// It asks the owner something, or a dialog waits for an answer.
    NeedsInput,
    /// At its prompt since `since` (seconds since 1970), asking nothing.
    Waiting {
        since: i64,
    },
}

/// A session that could take work, as the daemon sees it. Ended sessions are left out: a
/// placement on a session absent from the inputs ends.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub session: SessionId,
    /// The registered project its checkout belongs to; `None` outside every project.
    pub project: Option<String>,
    pub role: Option<Role>,
    /// What its provider reports; a step's required capabilities are checked against it.
    pub capabilities: Capabilities,
    pub presence: Presence,
}

/// What one pass works from besides the state database.
#[derive(Debug, Clone)]
pub struct Inputs {
    pub candidates: Vec<Candidate>,
    /// The session the owner is looking at in the manager's viewer: it gets no new work.
    pub viewed: Option<SessionId>,
    /// Seconds since 1970.
    pub now: i64,
}

/// What a waiting item waits as: the item itself, queued, or the ready step of the workflow
/// run it follows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unit {
    Item,
    Step {
        run: RunId,
        position: u32,
        name: &'static str,
    },
}

/// Why an item waits, as a stable name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Open but not queued: the owner queues it or hands it to a session.
    NotQueued,
    /// It waits on items that are not done.
    WaitsOnItems,
    /// Its workflow run is blocked on a next action.
    RunBlocked,
    /// Its workflow's current step is the owner's.
    OwnerStep,
    QueuePaused,
    ControlUnsupported,
    /// The agent turn can never fit the budget as configured.
    Infeasible,
    NoRoom,
    /// It would start, but an older item gathers the capacity first.
    Behind,
    /// No session works in its project.
    NoSession,
    /// Sessions work in its project, but none can take it now; each says why.
    NoSessionFree,
}

impl Reason {
    const ALL: [Reason; 11] = [
        Reason::NotQueued,
        Reason::WaitsOnItems,
        Reason::RunBlocked,
        Reason::OwnerStep,
        Reason::QueuePaused,
        Reason::ControlUnsupported,
        Reason::Infeasible,
        Reason::NoRoom,
        Reason::Behind,
        Reason::NoSession,
        Reason::NoSessionFree,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Reason::NotQueued => "not-queued",
            Reason::WaitsOnItems => "waits-on-items",
            Reason::RunBlocked => "run-blocked",
            Reason::OwnerStep => "owner-step",
            Reason::QueuePaused => "queue-paused",
            Reason::ControlUnsupported => "control-unsupported",
            Reason::Infeasible => "infeasible",
            Reason::NoRoom => "no-room",
            Reason::Behind => "behind",
            Reason::NoSession => "no-session",
            Reason::NoSessionFree => "no-session-free",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Reason> {
        Reason::ALL
            .into_iter()
            .find(|reason| reason.as_str() == name)
    }

    /// In the queue: placed once what holds it clears. The others wait on something outside
    /// scheduling first.
    pub fn is_queued(self) -> bool {
        !matches!(
            self,
            Reason::NotQueued | Reason::WaitsOnItems | Reason::RunBlocked | Reason::OwnerStep
        )
    }
}

/// Who does the available action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionBy {
    Owner,
    /// Nobody needs to: a later pass places it once capacity or a session frees up.
    Cockpit,
}

impl ActionBy {
    pub fn as_str(self) -> &'static str {
        match self {
            ActionBy::Owner => "owner",
            ActionBy::Cockpit => "cockpit",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<ActionBy> {
        [ActionBy::Owner, ActionBy::Cockpit]
            .into_iter()
            .find(|by| by.as_str() == name)
    }
}

/// Why one session of the item's project cannot take it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionNote {
    pub session: SessionId,
    pub why: String,
}

/// The record of one waiting item, rewritten by every pass (report 10, "queue
/// explanations"): why it waits, what limits it, what it would reserve, its priority and
/// aging, when it is looked at again, and what moves it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explanation {
    pub item: ItemId,
    pub project: String,
    pub title: String,
    pub unit: Unit,
    pub reason: Reason,
    /// The reason in a sentence.
    pub why: String,
    /// The resource and the capacity, the backend or the control that limits it.
    pub limited_by: Option<String>,
    /// For [`Reason::NoSessionFree`]: each session of the project and why it cannot take it.
    pub sessions: Vec<SessionNote>,
    /// For [`Reason::Behind`]: the item that gathers capacity first.
    pub behind: Option<ItemId>,
    pub action: String,
    pub action_by: ActionBy,
    /// The item's priority, 1 to 6.
    pub priority: i64,
    /// Levels gained by waiting; ordering uses `priority + aging_boost`.
    pub aging_boost: i64,
    /// What placing it would charge the budget, startup headroom included.
    pub demand: Demand,
    /// The startup headroom in `demand`, in percent of an agent turn.
    pub headroom_percent: u64,
    /// When it entered the queue; `None` outside it.
    pub waiting_since: Option<i64>,
    pub evaluated_at: i64,
    pub next_evaluation_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlacementId(pub i64);

impl fmt::Display for PlacementId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "placement {}", self.0)
    }
}

/// Work given to a session. It charges the budget until the work ends or the session goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub id: PlacementId,
    pub item: ItemId,
    pub unit: Unit,
    pub project: String,
    pub session: SessionId,
    pub demand: Demand,
    pub placed_at: i64,
    /// The admission reservation that charges it (#386); `None` for a placement made before
    /// placements held one, which the pass charges itself.
    pub reservation: Option<ReservationId>,
}

impl Placement {
    /// The one line typed to the session's agent to give it the work (#385): where to read
    /// it and how to report it done. The work itself is read from its durable records, so
    /// the line stays short and never goes stale.
    pub fn brief(&self) -> String {
        match &self.unit {
            Unit::Item => format!(
                "Cockpit gives you item {item}: read it with `cockpit todo show {item}`, do it, \
                 then finish it with `cockpit todo done {item} --summary '<what you did>'`.",
                item = self.item.0
            ),
            Unit::Step { run, name, .. } => format!(
                "Cockpit gives you step `{name}` of workflow run {run} (item {item}): read its \
                 task packet with `cockpit workflow packet {run}` and hand the step off as it \
                 says.",
                run = run.0,
                item = self.item.0
            ),
        }
    }
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dispatched {
    pub placed: Vec<Placement>,
    /// Placements whose work ended or whose session went since the last pass.
    pub ended: Vec<PlacementId>,
    /// Every open item still waiting, in dispatch order.
    pub explanations: Vec<Explanation>,
}
