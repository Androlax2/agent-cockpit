//! What this host can enforce, verified rather than assumed (report 10): the controllers
//! delegated to this user's systemd manager, the cgroup files of each scope, and the shims
//! that start a program idle or niced. A control is reported supported only with the
//! mechanism that enforces it, and unsupported with the missing piece.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cockpit_processes::{Containment, Execution, LaunchContainment, LaunchSpec, SystemdTools};
use rustix::process::{Pid, Signal};

use crate::{Availability, Control, ControlSupport, Demand, Error, Limits};

/// How long a host query (`systemctl`, `ps`, `sysctl`) may take before it counts as failed.
const HOST_COMMAND_DEADLINE: Duration = Duration::from_secs(5);
/// `SCHED_IDLE` in `/proc/<pid>/stat` field 41.
const SCHED_IDLE: &str = "5";

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
        }
    }
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

    /// What new executions get: the controls that hold work when required.
    pub fn support(&self) -> Vec<ControlSupport> {
        self.support_for(self.launch_containment == LaunchContainment::SystemdScope)
    }

    fn support_for(&self, in_scope: bool) -> Vec<ControlSupport> {
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
        if !in_scope {
            let why = match &self.cgroups {
                Cgroups::Unavailable { why } => why.clone(),
                Cgroups::Delegated { .. } => {
                    "this execution is a process group, which no cgroup holds".to_owned()
                }
            };
            return Availability::Unsupported { why };
        }
        match &self.cgroups {
            Cgroups::Unavailable { why } => Availability::Unsupported { why: why.clone() },
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

    fn supports(&self, control: Control, in_scope: bool) -> bool {
        matches!(
            self.availability(control, in_scope),
            Availability::Supported { .. }
        )
    }

    /// `spec` started through the shims `limits` asks for and this host has: `nice` and
    /// `chrt --idle`. Both act before the program runs, so every child inherits them.
    pub fn wrap(&self, spec: LaunchSpec, limits: &Limits) -> LaunchSpec {
        let mut prefix: Vec<OsString> = Vec::new();
        if let (Some(nice), Some(niceness)) = (&self.tools.nice, limits.nice) {
            prefix.extend([
                nice.clone().into_os_string(),
                "-n".into(),
                niceness.to_string().into(),
            ]);
        }
        if limits.idle_scheduling
            && self.supports(Control::IdleScheduling, false)
            && let Some(chrt) = &self.tools.chrt
        {
            prefix.extend([chrt.clone().into_os_string(), "--idle".into(), "0".into()]);
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
        let Containment::SystemdScope { unit } = &execution.containment else {
            return Ok(());
        };
        let properties: Vec<String> = [
            (
                Control::CpuWeight,
                format!(
                    "CPUWeight={}",
                    limits
                        .cpu_weight
                        .map_or("100".to_owned(), |w| w.to_string())
                ),
            ),
            (
                Control::CpuQuota,
                format!(
                    "CPUQuota={}",
                    limits
                        .cpu_quota_percent
                        .map_or(String::new(), |quota| format!("{quota}%"))
                ),
            ),
            (
                Control::MemoryMax,
                format!(
                    "MemoryMax={}",
                    limits
                        .memory_max_mib
                        .map_or("infinity".to_owned(), |mib| format!("{mib}M"))
                ),
            ),
            (
                Control::TasksMax,
                format!(
                    "TasksMax={}",
                    limits
                        .tasks_max
                        .map_or("infinity".to_owned(), |tasks| tasks.to_string())
                ),
            ),
        ]
        .into_iter()
        .filter(|(control, _)| self.supports(*control, true))
        .map(|(_, property)| property)
        .collect();
        if properties.is_empty() {
            return Ok(());
        }
        let mut arguments = vec!["--user", "set-property", "--runtime", unit.as_str()];
        arguments.extend(properties.iter().map(String::as_str));
        self.systemctl(&arguments).await.map(drop)
    }

    /// Requested, supported and effective for every control of `execution`, and what it
    /// uses now. Effective values are read from the kernel: the scope's cgroup files and the
    /// leader's scheduling fields.
    pub async fn inspect(&self, execution: &Execution, limits: &Limits) -> ExecutionReport {
        let scope_dir = match &execution.containment {
            Containment::SystemdScope { unit } => Some(self.scope_dir(unit).await),
            Containment::ProcessGroup => None,
        };
        let in_scope = scope_dir.is_some();
        let mut reports = Vec::new();
        for control in Control::ALL {
            let effective = match (&scope_dir, control) {
                (_, Control::IdleScheduling | Control::Nice) => {
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
            Containment::SystemdScope { unit } => {
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
        match self.platform {
            Platform::MacOs => macos_leader_effective(group, control).await,
            Platform::Linux | Platform::Wsl => {
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
        Control::IdleScheduling | Control::Nice => unreachable!("read from the leader"),
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
    // Field 2 (the command) may hold spaces and parentheses; it ends at the last `)`.
    let fields: Vec<&str> = stat
        .rsplit_once(") ")
        .map(|(_, rest)| rest.split_whitespace().collect())
        .unwrap_or_default();
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

async fn macos_leader_effective(leader: i32, control: Control) -> Effective {
    let column = match control {
        Control::Nice => "nice=",
        Control::Freeze => "stat=",
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
