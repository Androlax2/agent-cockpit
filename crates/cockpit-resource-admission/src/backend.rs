//! What this host can enforce, verified rather than assumed (report 10): the controllers
//! delegated to this user's systemd manager, the cgroup files of each scope, and the shims
//! that start a program idle or niced. A control is reported supported only with the
//! mechanism that enforces it, and unsupported with the missing piece.

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cockpit_processes::{Containment, Execution, LaunchContainment, LaunchSpec, SystemdTools};
use rustix::process::{Pid, Signal};

use crate::model::MAX_NICE;
use crate::{Availability, Control, ControlSupport, Demand, DiskSpace, Error, Limits, Policy};

/// How long a host query (`systemctl`, `ps`, `sysctl`) may take before it counts as failed.
const HOST_COMMAND_DEADLINE: Duration = Duration::from_secs(5);
/// `SCHED_IDLE` in `/proc/<pid>/stat` field 41.
const SCHED_IDLE: &str = "5";
/// The controls the daemon sets on a running session's processes (#1351).
const SESSION_CONTROLS: [Control; 5] = [
    Control::CpuWeight,
    Control::CpuQuota,
    Control::IdleScheduling,
    Control::Nice,
    Control::IdleIo,
];
/// How many times the ceilings walk a running tree: a process that forked between the read
/// of its tasks and their change has a child the next walk finds.
const TREE_WALKS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Linux,
    /// Windows runs Cockpit inside WSL 2 (report 01); what it enforces is probed like Linux.
    Wsl,
    MacOs,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Linux => "linux",
            Platform::Wsl => "wsl",
            Platform::MacOs => "macos",
        }
    }
}

/// Where the backend looks and what it runs. Paths, not lookups: the daemon resolves them
/// once and tests point them at fakes.
#[derive(Debug, Clone)]
pub struct HostTools {
    pub systemctl: PathBuf,
    /// The cgroup v2 mount, `/sys/fs/cgroup`.
    pub cgroup_root: PathBuf,
    /// `/proc`.
    pub proc_root: PathBuf,
    /// `chrt` (util-linux), which starts a program under `SCHED_IDLE`; `None` when missing.
    pub chrt: Option<PathBuf>,
    pub nice: Option<PathBuf>,
    /// `ionice` (util-linux), which starts a program in, or moves a task to, an IO class;
    /// `None` when missing or not on Linux.
    pub ionice: Option<PathBuf>,
    /// The niceness the daemon runs at, which its executions inherit: `nice -n` adds to it.
    pub own_niceness: i32,
    /// `RLIMIT_NICE`: how far below 20 an unprivileged daemon may lower a niceness, which
    /// it must to undo nice 19 and `SCHED_IDLE` on a running session. `None` when nothing
    /// limits it (root, or an unlimited soft limit).
    pub nice_rlimit: Option<u64>,
}

impl HostTools {
    /// This host's tools, with `systemctl` the one executions use.
    pub fn from_host(systemd: &SystemdTools) -> HostTools {
        HostTools {
            systemctl: systemd.systemctl.clone(),
            cgroup_root: PathBuf::from("/sys/fs/cgroup"),
            proc_root: PathBuf::from("/proc"),
            chrt: if cfg!(target_os = "linux") {
                find_program("chrt")
            } else {
                None
            },
            nice: find_program("nice"),
            ionice: if cfg!(target_os = "linux") {
                find_program("ionice")
            } else {
                None
            },
            // getpriority on the calling process has no failure case short of a broken libc;
            // 0 is the niceness every process starts from.
            own_niceness: rustix::process::getpriority_process(None).unwrap_or(0),
            nice_rlimit: own_nice_rlimit(),
        }
    }
}

#[cfg(target_os = "linux")]
fn own_nice_rlimit() -> Option<u64> {
    if rustix::process::geteuid().is_root() {
        return None;
    }
    rustix::process::getrlimit(rustix::process::Resource::Nice).current
}

/// macOS has no `RLIMIT_NICE`, and Torlek changes a running tree only on Linux.
#[cfg(not(target_os = "linux"))]
fn own_nice_rlimit() -> Option<u64> {
    Some(0)
}

/// The daemon's `PATH`, then the system directories: an execution's environment is cleared,
/// so the shim must be an absolute path.
fn find_program(name: &str) -> Option<PathBuf> {
    cockpit_processes::find_in_path(name).or_else(|| {
        ["/usr/bin", "/bin"]
            .into_iter()
            .map(|dir| Path::new(dir).join(name))
            .find(|candidate| candidate.is_file())
    })
}

/// `nice -n` adds to the niceness the daemon runs at, so the increment that lands an
/// execution on `target` is the difference. `None` when the daemon already runs at least as
/// nice: its children inherit that, and an unprivileged process cannot lower it.
fn nice_increment(target: u64, own_niceness: i32) -> Option<i64> {
    let increment = i64::try_from(target).ok()? - i64::from(own_niceness);
    (increment > 0).then_some(increment)
}

/// What the cgroup probe found for this user's manager.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Cgroups {
    Delegated {
        /// The manager's control group, as systemd names it.
        manager: String,
        controllers: Vec<String>,
        freezer: bool,
    },
    Unavailable {
        why: String,
    },
}

/// One host's enforcement backend, probed at daemon start.
#[derive(Debug, Clone)]
pub struct Backend {
    platform: Platform,
    tools: HostTools,
    launch_containment: LaunchContainment,
    cgroups: Cgroups,
    capacity: Demand,
    /// Shared by every clone of this backend; the daemon's disk guard writes it.
    disk_space: std::sync::Arc<std::sync::Mutex<Option<DiskSpace>>>,
}

/// Requested, supported and effective for one control of one execution (report 10).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitReport {
    pub control: Control,
    /// What Cockpit asks the backend for; `None` when nothing is asked.
    pub requested: Option<String>,
    pub availability: Availability,
    pub effective: Effective,
}

/// What the host shows is in force, read back from the kernel, never from the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effective {
    Value(String),
    Unknown { why: String },
}

/// What an execution uses now, where the backend can measure it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Observed {
    pub memory_bytes: Option<u64>,
    pub tasks: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionReport {
    pub limits: Vec<LimitReport>,
    pub observed: Observed,
}

impl Backend {
    /// Probes the host. `launch_containment` is what new executions use: a scope gets the
    /// controllers delegated to the manager, a process group none.
    pub async fn detect(
        tools: HostTools,
        launch_containment: LaunchContainment,
    ) -> Result<Backend, Error> {
        let platform = detect_platform(&tools.proc_root).await;
        let cgroups = match (platform, launch_containment) {
            (Platform::MacOs, _) => Cgroups::Unavailable {
                why: "macOS has no cgroups; executions run as process groups".to_owned(),
            },
            (_, LaunchContainment::ProcessGroup) => Cgroups::Unavailable {
                why: "no systemd user manager answered, so executions run as process groups, \
                      which no cgroup holds"
                    .to_owned(),
            },
            (_, LaunchContainment::SystemdScope) => probe_cgroups(&tools).await,
        };
        let capacity = Demand {
            cpu_millis: std::thread::available_parallelism()
                .map_err(Error::io("count the CPUs of", "this host"))?
                .get() as u64
                * 1000,
            memory_mib: memory_mib(&tools.proc_root).await?,
        };
        Ok(Backend {
            platform,
            tools,
            launch_containment,
            cgroups,
            capacity,
            disk_space: std::sync::Arc::default(),
        })
    }

    pub fn platform(&self) -> Platform {
        self.platform
    }

    pub fn launch_containment(&self) -> LaunchContainment {
        self.launch_containment
    }

    /// The host's CPUs and memory: the default budget.
    pub fn capacity(&self) -> Demand {
        self.capacity
    }

    /// Records the disk guard's last reading; every [`Backend::policy`] after it holds
    /// launches and heavy jobs while it is below the floor.
    pub fn set_disk_space(&self, space: Option<DiskSpace>) {
        *self
            .disk_space
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = space;
    }

    /// The disk guard's last reading; `None` before its first.
    pub fn disk_space(&self) -> Option<DiskSpace> {
        *self
            .disk_space
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The admission policy now: the host's budget and the disk guard's last reading.
    pub fn policy(&self) -> Policy {
        let mut policy = Policy::for_budget(self.capacity);
        policy.low_disk = self.disk_space().filter(|space| space.is_low());
        policy
    }

    /// What new executions get: the controls that hold work when required.
    pub fn support(&self) -> Vec<ControlSupport> {
        let in_scope = self.launch_containment == LaunchContainment::SystemdScope;
        Control::ALL
            .into_iter()
            .map(|control| ControlSupport {
                control,
                availability: self.availability(control, in_scope),
            })
            .collect()
    }

    fn availability(&self, control: Control, in_scope: bool) -> Availability {
        let supported = |how: String| Availability::Supported { how };
        let unsupported = |why: String| Availability::Unsupported { why };
        match control {
            Control::CpuWeight => self.cgroup_availability(in_scope, "cpu", "CPUWeight"),
            Control::CpuQuota => self.cgroup_availability(in_scope, "cpu", "CPUQuota"),
            Control::MemoryMax => self.cgroup_availability(in_scope, "memory", "MemoryMax"),
            Control::TasksMax => self.cgroup_availability(in_scope, "pids", "TasksMax"),
            Control::IdleScheduling => match (&self.tools.chrt, self.platform) {
                (_, Platform::MacOs) => {
                    unsupported("macOS has no SCHED_IDLE; nice is its subset".to_owned())
                }
                (Some(chrt), _) => supported(format!(
                    "{} --idle 0 starts the program; its children inherit SCHED_IDLE",
                    chrt.display()
                )),
                (None, _) => unsupported("chrt (util-linux) is not installed".to_owned()),
            },
            Control::Nice => match &self.tools.nice {
                Some(nice) => supported(format!(
                    "{} -n starts the program; its children inherit the niceness",
                    nice.display()
                )),
                None => unsupported("nice is not installed".to_owned()),
            },
            Control::IdleIo => match self.ionice() {
                Ok(ionice) => supported(format!(
                    "{} -c 3 starts the program; its children inherit the idle IO class",
                    ionice.display()
                )),
                Err(why) => unsupported(why),
            },
            Control::Freeze if !in_scope => supported(
                "SIGSTOP to the process group; a descendant that left it with setsid keeps \
                 running"
                    .to_owned(),
            ),
            Control::Freeze => match &self.cgroups {
                Cgroups::Delegated { freezer: true, .. } => {
                    supported("systemctl --user freeze: the cgroup v2 freezer".to_owned())
                }
                Cgroups::Delegated { manager, .. } => unsupported(format!(
                    "{manager} has no cgroup.freeze (kernel before 5.2)"
                )),
                Cgroups::Unavailable { why } => unsupported(why.clone()),
            },
        }
    }

    fn cgroup_availability(
        &self,
        in_scope: bool,
        controller: &str,
        property: &str,
    ) -> Availability {
        match &self.cgroups {
            Cgroups::Unavailable { why } => Availability::Unsupported { why: why.clone() },
            Cgroups::Delegated { .. } if !in_scope => Availability::Unsupported {
                why: "this execution is a process group, which no cgroup holds".to_owned(),
            },
            Cgroups::Delegated {
                manager,
                controllers,
                ..
            } if controllers.iter().any(|name| name == controller) => Availability::Supported {
                how: format!(
                    "{property}= on the execution's systemd scope ({controller} controller \
                     delegated to {manager})"
                ),
            },
            Cgroups::Delegated {
                manager,
                controllers,
                ..
            } => Availability::Unsupported {
                why: format!(
                    "the {controller} controller is not delegated to {manager} (delegated: {})",
                    if controllers.is_empty() {
                        "none".to_owned()
                    } else {
                        controllers.join(" ")
                    }
                ),
            },
        }
    }

    fn ionice(&self) -> Result<&Path, String> {
        match (&self.tools.ionice, self.platform) {
            (_, Platform::MacOs) => Err("macOS has no IO scheduling classes".to_owned()),
            (Some(ionice), _) => Ok(ionice),
            (None, _) => Err("ionice (util-linux) is not installed".to_owned()),
        }
    }

    /// What the daemon can set on a running session's processes, live and both ways (#1351).
    /// A ceiling that could not be taken off again is reported unsupported and never set.
    pub fn session_support(&self) -> Vec<ControlSupport> {
        SESSION_CONTROLS
            .into_iter()
            .map(|control| ControlSupport {
                control,
                availability: self.session_availability(control),
            })
            .collect()
    }

    fn session_availability(&self, control: Control) -> Availability {
        let supported = |how: String| Availability::Supported { how };
        let unsupported = |why: String| Availability::Unsupported { why };
        if self.platform == Platform::MacOs {
            return unsupported(
                "Torlek changes a running session's processes only on Linux".to_owned(),
            );
        }
        let in_scope = self.launch_containment == LaunchContainment::SystemdScope;
        match control {
            Control::CpuWeight => self.cgroup_availability(in_scope, "cpu", "CPUWeight"),
            Control::CpuQuota => self.cgroup_availability(in_scope, "cpu", "CPUQuota"),
            Control::IdleScheduling => match (&self.tools.chrt, self.leaves_idle_scheduling()) {
                (None, _) => unsupported("chrt (util-linux) is not installed".to_owned()),
                (_, Err(why)) => unsupported(why),
                (Some(chrt), Ok(())) => supported(format!(
                    "{} --idle -p on every task of the running session; --other undoes it",
                    chrt.display()
                )),
            },
            Control::Nice => supported(
                "setpriority on every task of the running session, in every profile".to_owned(),
            ),
            Control::IdleIo => match self.ionice() {
                Ok(ionice) => supported(format!(
                    "{} -c 3 -p on every task of the running session, in every profile",
                    ionice.display()
                )),
                Err(why) => unsupported(why),
            },
            other => unsupported(format!("{} is not a session ceiling", other.as_str())),
        }
    }

    /// Whether the daemon may move a session off `SCHED_IDLE` when the profile returns to
    /// normal: an unprivileged process may do so only within `RLIMIT_NICE`, which must allow
    /// the session's nice 19 (`20 - 19`).
    fn leaves_idle_scheduling(&self) -> Result<(), String> {
        let needed = 20 - MAX_NICE;
        match self.tools.nice_rlimit {
            Some(limit) if limit < needed => Err(format!(
                "RLIMIT_NICE is {limit}: the daemon could not take a running session off \
                 SCHED_IDLE when the profile returns to normal (it needs {needed}; raise the \
                 nice limit in limits.conf)"
            )),
            _ => Ok(()),
        }
    }

    fn supports_in_sessions(&self, control: Control) -> bool {
        matches!(
            self.session_availability(control),
            Availability::Supported { .. }
        )
    }

    fn supports(&self, control: Control, in_scope: bool) -> bool {
        matches!(
            self.availability(control, in_scope),
            Availability::Supported { .. }
        )
    }

    /// `spec` started through the shims `limits` asks for and this host has: `nice`,
    /// `chrt --idle` and `ionice -c 3`. They act before the program runs, so every child
    /// inherits them.
    pub fn wrap(&self, spec: LaunchSpec, limits: &Limits) -> LaunchSpec {
        let mut prefix: Vec<OsString> = Vec::new();
        if let (Some(nice), Some(increment)) = (
            &self.tools.nice,
            limits
                .nice
                .and_then(|target| nice_increment(target, self.tools.own_niceness)),
        ) {
            prefix.extend([
                nice.clone().into_os_string(),
                "-n".into(),
                increment.to_string().into(),
            ]);
        }
        if limits.idle_scheduling
            && self.supports(Control::IdleScheduling, false)
            && let Some(chrt) = &self.tools.chrt
        {
            prefix.extend([chrt.clone().into_os_string(), "--idle".into(), "0".into()]);
        }
        if limits.idle_io
            && let Ok(ionice) = self.ionice()
        {
            prefix.extend([ionice.as_os_str().to_owned(), "-c".into(), "3".into()]);
        }
        let Some((program, shim_arguments)) = prefix.split_first() else {
            return spec;
        };
        let mut arguments = shim_arguments.to_vec();
        arguments.push(spec.program.into_os_string());
        arguments.extend(spec.arguments);
        LaunchSpec {
            program: PathBuf::from(program),
            arguments,
            working_dir: spec.working_dir,
            environment: spec.environment,
            terminal: spec.terminal,
        }
    }

    /// Applies the cgroup limits `limits` asks for to a scope execution, and resets the ones
    /// it does not ask for, so a profile change both adds and removes ceilings. Controls the
    /// host does not enforce are skipped: [`Backend::inspect`] reports them. A process group
    /// has no cgroup limits to apply.
    pub async fn enforce(&self, execution: &Execution, limits: &Limits) -> Result<(), Error> {
        self.set_properties(execution, limits, &Control::ALL).await
    }

    /// Sets `controls`' cgroup properties on `execution`'s scope; nothing for a process group.
    async fn set_properties(
        &self,
        execution: &Execution,
        limits: &Limits,
        controls: &[Control],
    ) -> Result<(), Error> {
        let (Containment::SystemdScope { unit } | Containment::SystemdService { unit }) =
            &execution.containment
        else {
            return Ok(());
        };
        // An unrequested limit is reset to systemd's default, so a profile change also
        // removes the ceilings it added.
        let properties: Vec<String> = [
            (Control::CpuWeight, "CPUWeight", "100"),
            (Control::CpuQuota, "CPUQuota", ""),
            (Control::MemoryMax, "MemoryMax", "infinity"),
            (Control::TasksMax, "TasksMax", "infinity"),
        ]
        .into_iter()
        .filter(|(control, _, _)| controls.contains(control) && self.supports(*control, true))
        .map(|(control, property, default)| {
            let value = limits
                .requested(control)
                .unwrap_or_else(|| default.to_owned());
            format!("{property}={value}")
        })
        .collect();
        if properties.is_empty() {
            return Ok(());
        }
        let mut arguments = vec!["--user", "set-property", "--runtime", unit.as_str()];
        arguments.extend(properties.iter().map(String::as_str));
        self.systemctl(&arguments).await.map(drop)
    }

    /// Sets `limits` on a running session's whole tree (#1351), live and both ways: its
    /// scope's CPU weight and quota, and on every task its niceness, scheduling policy and IO
    /// class. A control off undoes what the foreground profile set: weight 100, no quota,
    /// `SCHED_OTHER` for an idle task, IO class none. A niceness is only ever raised, never
    /// lowered. Controls [`Backend::session_support`] reports unsupported are left alone. A
    /// task that ended meanwhile is skipped; every other failure is returned, after the rest
    /// of the tree changed.
    pub async fn apply_to_session(
        &self,
        execution: &Execution,
        limits: &Limits,
    ) -> Result<(), Error> {
        let mut failures = Vec::new();
        let cgroup_controls: Vec<Control> = [Control::CpuWeight, Control::CpuQuota]
            .into_iter()
            .filter(|control| self.supports_in_sessions(*control))
            .collect();
        if let Err(error) = self
            .set_properties(execution, limits, &cgroup_controls)
            .await
        {
            failures.push(error.to_string());
        }
        if self.platform == Platform::MacOs {
            return unapplied(execution, failures);
        }
        let source = match self.task_source(execution).await {
            Ok(source) => source,
            Err(error) => {
                failures.push(error.to_string());
                return unapplied(execution, failures);
            }
        };
        let mut changed = HashSet::new();
        for _ in 0..TREE_WALKS {
            let tasks = match self.tasks_in(&source).await {
                Ok(tasks) => tasks,
                Err(error) => {
                    failures.push(error.to_string());
                    break;
                }
            };
            let new: Vec<i32> = tasks
                .into_iter()
                .filter(|task| changed.insert(*task))
                .collect();
            if new.is_empty() {
                break;
            }
            self.set_tasks(&new, limits, &mut failures).await;
        }
        unapplied(execution, failures)
    }

    async fn set_tasks(&self, tasks: &[i32], limits: &Limits, failures: &mut Vec<String>) {
        for &task in tasks {
            let Some(now) = self.task_priority(task).await else {
                continue;
            };
            if let Some(niceness) = limits.nice.and_then(|nice| i32::try_from(nice).ok())
                && niceness > now.niceness
                && let Some(pid) = Pid::from_raw(task)
                && let Err(errno) = rustix::process::setpriority_process(Some(pid), niceness)
                && errno != rustix::io::Errno::SRCH
            {
                failures.push(format!("setpriority({task}, {niceness}): {errno}"));
            }
            if self.supports_in_sessions(Control::IdleScheduling)
                && let Some(chrt) = &self.tools.chrt
                && let Some(policy) = match (limits.idle_scheduling, now.idle) {
                    (true, false) => Some("--idle"),
                    (false, true) => Some("--other"),
                    _ => None,
                }
            {
                if policy == "--idle" && !self.task_leaves_idle_scheduling(task).await {
                    failures.push(format!(
                        "task {task} was left off SCHED_IDLE: its RLIMIT_NICE would keep it there"
                    ));
                } else if let Err(error) =
                    host_command(chrt, &[policy, "-p", "0", &task.to_string()]).await
                    && self.task_priority(task).await.is_some()
                {
                    failures.push(error.to_string());
                }
            }
        }
        if self.supports_in_sessions(Control::IdleIo)
            && let Ok(ionice) = self.ionice()
        {
            self.set_io_class(ionice, tasks, limits.idle_io, failures)
                .await;
        }
    }

    /// One `ionice` for the whole batch; when it fails (it stops at the first task that
    /// ended), one per task, skipping those that ended.
    async fn set_io_class(
        &self,
        ionice: &Path,
        tasks: &[i32],
        is_idle: bool,
        failures: &mut Vec<String>,
    ) {
        let class = if is_idle { "3" } else { "0" };
        let task_names: Vec<String> = tasks.iter().map(i32::to_string).collect();
        let mut arguments = vec!["-c", class, "-p"];
        arguments.extend(task_names.iter().map(String::as_str));
        if host_command(ionice, &arguments).await.is_ok() {
            return;
        }
        for (task, name) in tasks.iter().zip(&task_names) {
            if let Err(error) = host_command(ionice, &["-c", class, "-p", name]).await
                && self.task_priority(*task).await.is_some()
            {
                failures.push(error.to_string());
            }
        }
    }

    /// Whether the daemon could take `task` off `SCHED_IDLE` again: the kernel checks the
    /// task's own `RLIMIT_NICE`, which a process may lower below the daemon's.
    async fn task_leaves_idle_scheduling(&self, task: i32) -> bool {
        if self.tools.nice_rlimit.is_none() {
            return true;
        }
        let limits = self.tools.proc_root.join(task.to_string()).join("limits");
        let Ok(limits) = tokio::fs::read_to_string(limits).await else {
            return false;
        };
        limits
            .lines()
            .find_map(|line| line.strip_prefix("Max nice priority"))
            .and_then(|values| values.split_whitespace().next())
            .is_some_and(|soft| {
                soft == "unlimited" || soft.parse::<u64>().is_ok_and(|soft| soft >= 20 - MAX_NICE)
            })
    }

    /// The niceness and policy of one task, from `/proc/<task>/stat`; `None` once it ended.
    async fn task_priority(&self, task: i32) -> Option<TaskPriority> {
        let stat =
            tokio::fs::read_to_string(self.tools.proc_root.join(task.to_string()).join("stat"))
                .await
                .ok()?;
        let fields = stat_fields(&stat);
        Some(TaskPriority {
            niceness: fields.get(19 - 3)?.parse().ok()?,
            idle: *fields.get(41 - 3)? == SCHED_IDLE,
        })
    }

    /// Where the tasks of a running execution are listed, resolved once per change.
    async fn task_source(&self, execution: &Execution) -> Result<TaskSource, Error> {
        if let Containment::SystemdScope { unit } | Containment::SystemdService { unit } =
            &execution.containment
        {
            let dir = self
                .scope_dir(unit)
                .await
                .map_err(|reason| Error::HostCommand {
                    command: format!("systemctl --user show {unit}"),
                    reason,
                })?;
            return Ok(TaskSource::Threads(dir.join("cgroup.threads")));
        }
        let group = execution.process_group.ok_or(Error::NotStarted {
            execution: execution.id,
        })?;
        Ok(TaskSource::Tree {
            group: group.to_string(),
        })
    }

    /// Every task (thread) of a running execution: its scope's `cgroup.threads`, or for a
    /// process group every task of a process in that group or session, or descended from
    /// one (a child that called `setsid` stays in the tree).
    async fn tasks_in(&self, source: &TaskSource) -> Result<Vec<i32>, Error> {
        let group = match source {
            TaskSource::Threads(listed) => {
                let threads = tokio::fs::read_to_string(listed)
                    .await
                    .map_err(Error::io("read", listed))?;
                return Ok(threads
                    .split_whitespace()
                    .filter_map(|task| task.parse().ok())
                    .collect());
            }
            TaskSource::Tree { group } => group,
        };
        let mut members = HashSet::new();
        let mut parents = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.tools.proc_root)
            .await
            .map_err(Error::io("list", &self.tools.proc_root))?;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(Error::io("list", &self.tools.proc_root))?
        {
            let Some(process) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
            else {
                continue;
            };
            let Ok(stat) = tokio::fs::read_to_string(entry.path().join("stat")).await else {
                continue;
            };
            let fields = stat_fields(&stat);
            if [5, 6]
                .into_iter()
                .any(|field| fields.get(field - 3) == Some(&group.as_str()))
            {
                members.insert(process);
            } else if let Some(parent) = fields.get(4 - 3).and_then(|parent| parent.parse().ok()) {
                parents.push((process, parent));
            }
        }
        while let Some(found) = parents
            .iter()
            .position(|(_, parent)| members.contains(parent))
        {
            members.insert(parents.swap_remove(found).0);
        }
        let mut tasks = Vec::new();
        for process in members {
            let threads_dir = self.tools.proc_root.join(process.to_string()).join("task");
            let Ok(mut threads) = tokio::fs::read_dir(threads_dir).await else {
                continue;
            };
            let before = tasks.len();
            while let Ok(Some(thread)) = threads.next_entry().await {
                tasks.extend(
                    thread
                        .file_name()
                        .to_str()
                        .and_then(|name| name.parse::<i32>().ok()),
                );
            }
            if tasks.len() == before {
                tasks.push(process);
            }
        }
        Ok(tasks)
    }

    /// Requested, supported and effective for the controls a running session gets: what
    /// `resources status` shows for each session (#1351).
    pub async fn inspect_session(&self, execution: &Execution, limits: &Limits) -> ExecutionReport {
        let mut report = self.inspect(execution, limits).await;
        report
            .limits
            .retain(|line| SESSION_CONTROLS.contains(&line.control));
        for line in &mut report.limits {
            line.availability = self.session_availability(line.control);
        }
        report
    }

    /// The report for work this backend cannot look at (not started, or its execution
    /// unreadable): what is requested and supported for new executions, every effective
    /// value unknown with `why`.
    pub fn unknown_report(&self, limits: &Limits, why: &str) -> ExecutionReport {
        ExecutionReport {
            limits: self
                .support()
                .into_iter()
                .map(|support| LimitReport {
                    control: support.control,
                    requested: limits.requested(support.control),
                    availability: support.availability,
                    effective: Effective::Unknown {
                        why: why.to_owned(),
                    },
                })
                .collect(),
            observed: Observed::default(),
        }
    }

    /// Requested, supported and effective for every control of `execution`, and what it
    /// uses now. Effective values are read from the kernel: the scope's cgroup files and the
    /// leader's scheduling fields.
    pub async fn inspect(&self, execution: &Execution, limits: &Limits) -> ExecutionReport {
        let scope_dir = match &execution.containment {
            Containment::SystemdScope { unit } | Containment::SystemdService { unit } => {
                Some(self.scope_dir(unit).await)
            }
            Containment::ProcessGroup => None,
        };
        let in_scope = scope_dir.is_some();
        let mut reports = Vec::new();
        for control in Control::ALL {
            let effective = match (&scope_dir, control) {
                (_, Control::IdleScheduling | Control::Nice | Control::IdleIo) => {
                    self.leader_effective(execution, control).await
                }
                (None, Control::Freeze) => self.leader_effective(execution, control).await,
                (None, _) => Effective::Unknown {
                    why: "a process group has no cgroup".to_owned(),
                },
                (Some(Err(why)), _) => Effective::Unknown { why: why.clone() },
                (Some(Ok(dir)), _) => cgroup_effective(dir, control).await,
            };
            reports.push(LimitReport {
                control,
                requested: limits.requested(control),
                availability: self.availability(control, in_scope),
                effective,
            });
        }
        let observed = match &scope_dir {
            Some(Ok(dir)) => Observed {
                memory_bytes: read_number(&dir.join("memory.current")).await,
                tasks: read_number(&dir.join("pids.current")).await,
            },
            _ => Observed::default(),
        };
        ExecutionReport {
            limits: reports,
            observed,
        }
    }

    /// Stops every process of the execution where it is; it still holds its memory and its
    /// locks (report 10). A scope uses the cgroup freezer, a process group `SIGSTOP`.
    pub async fn freeze(&self, execution: &Execution) -> Result<(), Error> {
        self.set_frozen(execution, true).await
    }

    pub async fn thaw(&self, execution: &Execution) -> Result<(), Error> {
        self.set_frozen(execution, false).await
    }

    async fn set_frozen(&self, execution: &Execution, frozen: bool) -> Result<(), Error> {
        match &execution.containment {
            Containment::SystemdScope { unit } | Containment::SystemdService { unit } => {
                let verb = if frozen { "freeze" } else { "thaw" };
                self.systemctl(&["--user", verb, unit]).await.map(drop)
            }
            Containment::ProcessGroup => {
                let group =
                    execution
                        .process_group
                        .and_then(Pid::from_raw)
                        .ok_or(Error::NotStarted {
                            execution: execution.id,
                        })?;
                let signal = if frozen { Signal::STOP } else { Signal::CONT };
                rustix::process::kill_process_group(group, signal).map_err(|errno| {
                    Error::HostCommand {
                        command: format!("kill -{signal:?} -{}", group.as_raw_nonzero()),
                        reason: errno.to_string(),
                    }
                })
            }
        }
    }

    /// The scope's cgroup directory, or why it cannot be read.
    async fn scope_dir(&self, unit: &str) -> Result<PathBuf, String> {
        let group = self
            .systemctl(&["--user", "show", "--property=ControlGroup", "--value", unit])
            .await
            .map_err(|error| error.to_string())?;
        let group = group.trim();
        if group.is_empty() {
            return Err(format!("{unit} has no control group: it is not running"));
        }
        Ok(self.tools.cgroup_root.join(group.trim_start_matches('/')))
    }

    async fn leader_effective(&self, execution: &Execution, control: Control) -> Effective {
        let Some(group) = execution.process_group else {
            return Effective::Unknown {
                why: "the execution has not started".to_owned(),
            };
        };
        match (self.platform, control) {
            (Platform::MacOs, _) => macos_leader_effective(group, control).await,
            (Platform::Linux | Platform::Wsl, Control::IdleIo) => match self.ionice() {
                Ok(ionice) => match host_command(ionice, &["-p", &group.to_string()]).await {
                    Ok(class) => Effective::Value(class.trim().to_owned()),
                    Err(error) => Effective::Unknown {
                        why: error.to_string(),
                    },
                },
                Err(why) => Effective::Unknown { why },
            },
            (Platform::Linux | Platform::Wsl, _) => {
                linux_leader_effective(&self.tools.proc_root, group, control).await
            }
        }
    }

    async fn systemctl(&self, arguments: &[&str]) -> Result<String, Error> {
        host_command(&self.tools.systemctl, arguments).await
    }
}

async fn detect_platform(proc_root: &Path) -> Platform {
    if cfg!(target_os = "macos") {
        return Platform::MacOs;
    }
    let release = tokio::fs::read_to_string(proc_root.join("sys/kernel/osrelease"))
        .await
        .unwrap_or_default();
    if release.to_lowercase().contains("microsoft") {
        Platform::Wsl
    } else {
        Platform::Linux
    }
}

/// The controllers this user's manager may hand to its scopes: its own `cgroup.controllers`.
async fn probe_cgroups(tools: &HostTools) -> Cgroups {
    let manager = match host_command(
        &tools.systemctl,
        &["--user", "show", "--property=ControlGroup", "--value"],
    )
    .await
    {
        Ok(group) if !group.trim().is_empty() => group.trim().to_owned(),
        Ok(_) => {
            return Cgroups::Unavailable {
                why: "the systemd user manager reports no control group".to_owned(),
            };
        }
        Err(error) => {
            return Cgroups::Unavailable {
                why: format!("cannot ask the systemd user manager: {error}"),
            };
        }
    };
    let dir = tools.cgroup_root.join(manager.trim_start_matches('/'));
    let listed = dir.join("cgroup.controllers");
    match tokio::fs::read_to_string(&listed).await {
        Ok(controllers) => Cgroups::Delegated {
            controllers: controllers.split_whitespace().map(str::to_owned).collect(),
            freezer: tokio::fs::try_exists(dir.join("cgroup.freeze"))
                .await
                .unwrap_or(false),
            manager,
        },
        Err(error) => Cgroups::Unavailable {
            why: format!(
                "cannot read {}: {error} (cgroup v2 is not mounted there)",
                listed.display()
            ),
        },
    }
}

async fn cgroup_effective(dir: &Path, control: Control) -> Effective {
    let file = match control {
        Control::CpuWeight => "cpu.weight",
        Control::CpuQuota => "cpu.max",
        Control::MemoryMax => "memory.max",
        Control::TasksMax => "pids.max",
        Control::Freeze => "cgroup.freeze",
        Control::IdleScheduling | Control::Nice | Control::IdleIo => {
            unreachable!("read from the leader")
        }
    };
    let text = match tokio::fs::read_to_string(dir.join(file)).await {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Effective::Unknown {
                why: format!("{file} is absent: the controller is not enabled on this scope"),
            };
        }
        Err(error) => {
            return Effective::Unknown {
                why: format!("cannot read {file}: {error}"),
            };
        }
    };
    let text = text.trim();
    let value = match control {
        Control::CpuQuota => match text.split_once(' ') {
            Some(("max", _)) => Some("none".to_owned()),
            Some((quota, period)) => quota
                .parse::<u64>()
                .ok()
                .zip(period.parse::<u64>().ok().filter(|period| *period > 0))
                .map(|(quota, period)| format!("{}%", quota * 100 / period)),
            None => None,
        },
        Control::MemoryMax => match text {
            "max" => Some("none".to_owned()),
            bytes => bytes
                .parse::<u64>()
                .ok()
                .map(|bytes| format!("{}M", bytes / (1024 * 1024))),
        },
        Control::TasksMax => match text {
            "max" => Some("none".to_owned()),
            tasks => tasks.parse::<u64>().ok().map(|tasks| tasks.to_string()),
        },
        Control::Freeze => match text {
            "1" => Some("frozen".to_owned()),
            "0" => Some("thawed".to_owned()),
            _ => None,
        },
        _ => Some(text.to_owned()),
    };
    match value {
        Some(value) => Effective::Value(value),
        None => Effective::Unknown {
            why: format!("{file} holds {text:?}"),
        },
    }
}

/// Nice, scheduling policy and stopped state of the group's leader: `/proc/<pid>/stat`
/// fields 19, 41 and 3.
async fn linux_leader_effective(proc_root: &Path, leader: i32, control: Control) -> Effective {
    let path = proc_root.join(leader.to_string()).join("stat");
    let stat = match tokio::fs::read_to_string(&path).await {
        Ok(stat) => stat,
        Err(error) => {
            return Effective::Unknown {
                why: format!("cannot read {}: {error}", path.display()),
            };
        }
    };
    let fields = stat_fields(&stat);
    let field = |number: usize| fields.get(number - 3).copied();
    let value = match control {
        Control::Nice => field(19).map(str::to_owned),
        Control::IdleScheduling => field(41).map(|policy| {
            if policy == SCHED_IDLE {
                "idle".to_owned()
            } else {
                format!("not idle (policy {policy})")
            }
        }),
        Control::Freeze => field(3).map(|state| {
            if state == "T" {
                "frozen".to_owned()
            } else {
                "thawed".to_owned()
            }
        }),
        _ => None,
    };
    value.map_or_else(
        || Effective::Unknown {
            why: format!("{} has too few fields", path.display()),
        },
        Effective::Value,
    )
}

/// The fields of a `/proc/<pid>/stat` from field 3 on: field 2 (the command) may hold
/// spaces and parentheses, and ends at the last `)`.
fn stat_fields(stat: &str) -> Vec<&str> {
    stat.rsplit_once(") ")
        .map(|(_, rest)| rest.split_whitespace().collect())
        .unwrap_or_default()
}

/// One task's CPU priority as `/proc` shows it.
enum TaskSource {
    /// A scope's `cgroup.threads`.
    Threads(PathBuf),
    /// The processes of a process group or session, and their descendants.
    Tree { group: String },
}

struct TaskPriority {
    niceness: i32,
    idle: bool,
}

/// `Ok` when nothing failed, or every failure of one change of a running tree.
fn unapplied(execution: &Execution, failures: Vec<String>) -> Result<(), Error> {
    if failures.is_empty() {
        return Ok(());
    }
    Err(Error::Unapplied {
        execution: execution.id,
        failures,
    })
}

async fn macos_leader_effective(leader: i32, control: Control) -> Effective {
    let column = match control {
        Control::Nice => "nice=",
        Control::Freeze => "stat=",
        Control::IdleIo => {
            return Effective::Unknown {
                why: "macOS has no IO scheduling classes".to_owned(),
            };
        }
        _ => {
            return Effective::Unknown {
                why: "macOS has no SCHED_IDLE".to_owned(),
            };
        }
    };
    let ps = Path::new("/bin/ps");
    match host_command(ps, &["-o", column, "-p", &leader.to_string()]).await {
        Ok(text) if control == Control::Freeze => Effective::Value(
            if text.trim().starts_with('T') {
                "frozen"
            } else {
                "thawed"
            }
            .to_owned(),
        ),
        Ok(text) => Effective::Value(text.trim().to_owned()),
        Err(error) => Effective::Unknown {
            why: error.to_string(),
        },
    }
}

async fn read_number(path: &Path) -> Option<u64> {
    tokio::fs::read_to_string(path)
        .await
        .ok()?
        .trim()
        .parse()
        .ok()
}

async fn memory_mib(proc_root: &Path) -> Result<u64, Error> {
    if cfg!(target_os = "macos") {
        let bytes = host_command(Path::new("/usr/sbin/sysctl"), &["-n", "hw.memsize"]).await?;
        return bytes
            .trim()
            .parse::<u64>()
            .map(|bytes| bytes / (1024 * 1024))
            .map_err(|error| Error::HostCommand {
                command: "sysctl -n hw.memsize".to_owned(),
                reason: format!("{bytes:?}: {error}"),
            });
    }
    let path = proc_root.join("meminfo");
    let meminfo = tokio::fs::read_to_string(&path)
        .await
        .map_err(Error::io("read", &path))?;
    meminfo
        .lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.trim().strip_suffix("kB"))
        .and_then(|kib| kib.trim().parse::<u64>().ok())
        .map(|kib| kib / 1024)
        .ok_or_else(|| Error::HostCommand {
            command: format!("read {}", path.display()),
            reason: "no MemTotal line in kB".to_owned(),
        })
}

/// Runs a host query with a deadline and returns its stdout, failing loud on a non-zero exit.
async fn host_command(program: &Path, arguments: &[&str]) -> Result<String, Error> {
    let command_line = || format!("{} {}", program.display(), arguments.join(" "));
    let output = tokio::process::Command::new(program)
        .args(arguments)
        .env("LC_ALL", "C")
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(HOST_COMMAND_DEADLINE, output)
        .await
        .map_err(|_| Error::HostCommand {
            command: command_line(),
            reason: format!("no answer within {HOST_COMMAND_DEADLINE:?}"),
        })?
        .map_err(|source| Error::HostCommand {
            command: command_line(),
            reason: source.to_string(),
        })?;
    if !output.status.success() {
        return Err(Error::HostCommand {
            command: command_line(),
            reason: format!(
                "{}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
