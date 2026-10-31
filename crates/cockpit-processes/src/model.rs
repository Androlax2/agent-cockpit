use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

/// One execution's row id. An execution is one OS process group or systemd scope; its id
/// never changes, whatever happens to the processes in it (report 04).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ExecutionId(pub i64);

impl fmt::Display for ExecutionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "execution {}", self.0)
    }
}

/// What holds an execution's whole process tree, so termination reaches every descendant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Containment {
    /// A transient systemd user scope: its cgroup holds every descendant, including one
    /// that leaves the process group with `setsid`.
    SystemdScope { unit: String },
    /// A process group led by the launched program. A descendant that calls `setsid`
    /// leaves it; that is the macOS limit report 03 accepts.
    ProcessGroup,
}

pub(crate) const SYSTEMD_SCOPE: &str = "systemd-scope";
pub(crate) const PROCESS_GROUP: &str = "process-group";

/// How the processes of an execution are recognised after a daemon restart: never by PID
/// alone (report 04). A process group id is reused only once the group is empty, and the
/// leader's start token tells a reused leader PID apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub containment: Containment,
    /// `/proc/sys/kernel/random/boot_id` on Linux, `kern.bootsessionuuid` on macOS: no
    /// process of an earlier boot survives.
    pub boot_id: String,
    pub process_group: i32,
    /// The leader's start time as the platform reports it: clock ticks since boot (Linux,
    /// `/proc/<pid>/stat` field 22) or `ps -o lstart` (macOS). Compared, never converted.
    pub leader_start: String,
}

/// Where an execution is in its life. Cancellation is a separate fact (report 04).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// The launch intent is committed; the launcher has not yet confirmed that the program
    /// started. The execution occupies its reservation.
    Launching,
    /// The program started and its identity is recorded.
    Running,
    /// No process of the execution remains; only an observation records this.
    Ended(EndReason),
}

impl Phase {
    /// Whether the execution still holds its reservation. Only an observed end releases it,
    /// never a dropped task or a timed-out launcher (report 03).
    pub fn is_occupying(&self) -> bool {
        !matches!(self, Phase::Ended(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// The execution's processes all exited without a cancel request.
    Exited,
    /// The execution was observed empty after a cancel request.
    Terminated,
    /// The launcher never started the program.
    NeverStarted,
    /// The host rebooted since the launch: no process of that boot survives.
    HostRestarted,
}

impl EndReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            EndReason::Exited => "exited",
            EndReason::Terminated => "terminated",
            EndReason::NeverStarted => "never-started",
            EndReason::HostRestarted => "host-restarted",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<EndReason> {
        [
            EndReason::Exited,
            EndReason::Terminated,
            EndReason::NeverStarted,
            EndReason::HostRestarted,
        ]
        .into_iter()
        .find(|reason| reason.as_str() == name)
    }
}

/// An execution as the state database records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Execution {
    pub id: ExecutionId,
    pub containment: Containment,
    pub boot_id: String,
    pub phase: Phase,
    /// Known once the launcher spawned; `None` only between the intent and the spawn.
    pub process_group: Option<i32>,
    /// Known once the program confirmed its start.
    pub leader_start: Option<String>,
    pub cancel_requested: bool,
}

impl Execution {
    pub fn identity(&self) -> Option<Identity> {
        Some(Identity {
            containment: self.containment.clone(),
            boot_id: self.boot_id.clone(),
            process_group: self.process_group?,
            leader_start: self.leader_start.clone()?,
        })
    }
}

/// What to run. The environment is exactly `environment`: nothing of the daemon's own
/// environment leaks into an execution unless a caller passes it on purpose.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    pub program: PathBuf,
    pub arguments: Vec<OsString>,
    pub working_dir: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
}

/// How long termination waits before escalating, and before giving up on confirmation.
#[derive(Debug, Clone, Copy)]
pub struct Escalation {
    /// After `SIGTERM`, how long the execution may take to empty before `SIGKILL`.
    pub grace: Duration,
    /// After `SIGKILL`, how long to wait for the observed-empty confirmation.
    pub kill_wait: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
}

/// The two facts of a stop request (report 03): the request is always recorded; the
/// termination counts only once the execution was observed empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Termination {
    /// Observed empty; `after` is the last signal sent, `None` when it was already empty.
    Confirmed { after: Option<Signal> },
    /// Still not observed empty after `SIGKILL` and `kill_wait`, or no identity to signal
    /// yet: the execution keeps its reservation.
    Unconfirmed,
}

/// What startup reconciliation decided for one execution (report 03, restart contract).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reconciled {
    /// Positively identified as still running: reattached, reservation kept.
    Reattached(ExecutionId),
    /// Observed gone: the reservation is released with this reason.
    Ended(ExecutionId, EndReason),
    /// Neither identified nor observed gone: the reservation stays charged.
    Uncertain(ExecutionId),
}
