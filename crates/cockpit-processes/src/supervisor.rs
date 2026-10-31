//! Launch, observe, terminate and reconcile executions against the real processes.

use std::fs::{DirBuilder, OpenOptions};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use cockpit_turso::Database;
use tokio::sync::{OnceCell, oneshot};

use crate::model::{PROCESS_GROUP, SYSTEMD_SCOPE};
use crate::output::{OutputKind, OutputLimits, OutputStream};
use crate::platform::{self, LAUNCH_SCRIPT};
use crate::systemd::SystemdTools;
use crate::{
    Containment, EndReason, Error, Escalation, Execution, ExecutionId, Identity, LaunchSpec, Phase,
    Reconciled, Signal, Termination, store,
};

const LEADER_FILE: &str = "leader";
/// How often a wait for a launch confirmation or for an empty execution looks again.
const OBSERVE_INTERVAL: Duration = Duration::from_millis(20);
/// How often a followed stream checks whether its execution emptied. Slower than
/// [`OBSERVE_INTERVAL`]: for a scope each check spawns `systemctl`.
const PRESENCE_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// The launcher's stderr quoted in [`Error::LaunchFailed`].
const LAUNCHER_OUTPUT_QUOTE_BYTES: usize = 2048;

/// Which containment new launches use. Recorded per execution: a restart reconciles each
/// execution with the containment it was launched in, whatever new launches use now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchContainment {
    SystemdScope,
    ProcessGroup,
}

/// Owns this daemon's executions: their files under `executions_dir` and their rows.
#[derive(Debug, Clone)]
pub struct Supervisor {
    executions_dir: PathBuf,
    launch_containment: LaunchContainment,
    systemd: SystemdTools,
    /// How long a launch waits for the program to confirm its start.
    pub launch_timeout: Duration,
    pub output_limits: OutputLimits,
    /// Read once: the boot id cannot change while this process runs.
    boot_id: Arc<OnceCell<String>>,
}

/// What the system shows of an execution's processes right now.
enum Presence {
    /// Positively identified; `confirmed` is the leader file once the program wrote it.
    Ours {
        confirmed: Option<(i32, String)>,
    },
    Gone(EndReason),
    /// Neither identified nor observed gone; the reservation stays charged.
    Unidentified,
}

impl Supervisor {
    pub fn new(
        executions_dir: PathBuf,
        launch_containment: LaunchContainment,
        systemd: SystemdTools,
    ) -> Supervisor {
        Supervisor {
            executions_dir,
            launch_containment,
            systemd,
            launch_timeout: Duration::from_secs(10),
            output_limits: OutputLimits::default(),
            boot_id: Arc::new(OnceCell::new()),
        }
    }

    async fn boot_id(&self) -> Result<&str, Error> {
        let boot_id = self.boot_id.get_or_try_init(platform::boot_id).await?;
        Ok(boot_id)
    }

    /// A systemd scope per execution on Linux when this user's manager answers; a process
    /// group otherwise, and always on macOS (report 03; docs/DECISIONS.md, 2026-10-31).
    pub async fn detect(executions_dir: PathBuf, systemd: SystemdTools) -> Supervisor {
        let containment = if cfg!(target_os = "linux") && systemd.manager_reachable().await {
            LaunchContainment::SystemdScope
        } else {
            LaunchContainment::ProcessGroup
        };
        Supervisor::new(executions_dir, containment, systemd)
    }

    pub fn launch_containment(&self) -> LaunchContainment {
        self.launch_containment
    }

    pub fn output_path(&self, database: &Database, id: ExecutionId, kind: OutputKind) -> PathBuf {
        self.execution_dir(database, id).join(kind.file_name())
    }

    /// `<executions dir>/<database epoch>/<id>`: a restored database starts a new epoch and
    /// may count ids again from a lower one, so ids alone would collide with old files.
    pub fn execution_dir(&self, database: &Database, id: ExecutionId) -> PathBuf {
        self.executions_dir
            .join(database.epoch())
            .join(id.0.to_string())
    }

    /// Commits the launch intent, spawns the program in its own containment and waits for its
    /// start confirmation. On [`Error::LaunchTimedOut`] the execution keeps its reservation
    /// and may still start; [`Supervisor::observe`] or [`Supervisor::reconcile`] decides.
    pub async fn launch(&self, database: &Database, spec: &LaunchSpec) -> Result<Execution, Error> {
        let boot_id = self.boot_id().await?.to_owned();
        let containment_kind = match self.launch_containment {
            LaunchContainment::SystemdScope => SYSTEMD_SCOPE,
            LaunchContainment::ProcessGroup => PROCESS_GROUP,
        };
        let unit_prefix = format!("cockpit-{}", database.epoch());
        let (id, containment) =
            store::record_intent(database, containment_kind, &unit_prefix, &boot_id).await?;
        let mut launcher = match self.spawn(self.execution_dir(database, id), &containment, spec) {
            Ok(launcher) => launcher,
            Err(error) => {
                store::record_ended(database, id, EndReason::NeverStarted).await?;
                return Err(error);
            }
        };
        let process_group = launcher
            .id()
            .and_then(|pid| i32::try_from(pid).ok())
            .expect("a just-spawned child has a pid");
        store::record_process_group(database, id, process_group).await?;
        // The daemon reaps its own children, so an exited leader never lingers as a zombie
        // that keeps its process group alive. After a restart, init reaps them.
        let (exited_sender, launcher_exited) = oneshot::channel();
        tokio::spawn(async move {
            let _ = exited_sender.send(launcher.wait().await);
        });
        self.confirm_start(database, id, process_group, launcher_exited)
            .await
    }

    fn spawn(
        &self,
        dir: PathBuf,
        containment: &Containment,
        spec: &LaunchSpec,
    ) -> Result<tokio::process::Child, Error> {
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(Error::io("create", &dir))?;
        let output_file = |kind: OutputKind| {
            let path = dir.join(kind.file_name());
            OpenOptions::new()
                .append(true)
                .create_new(true)
                .open(&path)
                .map_err(Error::io("create", path))
        };
        let mut command = match containment {
            Containment::SystemdScope { unit } => {
                let mut command = tokio::process::Command::new(&self.systemd.systemd_run);
                command.args(SystemdTools::scope_arguments(unit));
                command.arg("/bin/sh");
                command
            }
            Containment::ProcessGroup => tokio::process::Command::new("/bin/sh"),
        };
        command
            .arg("-c")
            .arg(LAUNCH_SCRIPT)
            .arg("cockpit-launch")
            .arg(dir.join(LEADER_FILE))
            .arg(&spec.program)
            .args(&spec.arguments)
            .current_dir(&spec.working_dir)
            .env_clear()
            .envs(spec.environment.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .stdout(output_file(OutputKind::Stdout)?)
            .stderr(output_file(OutputKind::Stderr)?)
            .process_group(0);
        if let Containment::SystemdScope { .. } = containment {
            // `systemd-run --user` finds this user's manager through these; with `--scope`
            // it execs the program, which inherits them. A spec's own value wins.
            for name in ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"] {
                let spec_sets_it = spec.environment.iter().any(|(set, _)| set == name);
                if let (false, Some(value)) = (spec_sets_it, std::env::var_os(name)) {
                    command.env(name, value);
                }
            }
        }
        let program = match containment {
            Containment::SystemdScope { .. } => self.systemd.systemd_run.clone(),
            Containment::ProcessGroup => PathBuf::from("/bin/sh"),
        };
        command.spawn().map_err(Error::io("spawn", program))
    }

    async fn confirm_start(
        &self,
        database: &Database,
        id: ExecutionId,
        process_group: i32,
        mut launcher_exited: oneshot::Receiver<std::io::Result<std::process::ExitStatus>>,
    ) -> Result<Execution, Error> {
        let leader_path = self.execution_dir(database, id).join(LEADER_FILE);
        let deadline = tokio::time::Instant::now() + self.launch_timeout;
        loop {
            if let Some((pid, start)) = read_leader_file(&leader_path).await? {
                if pid != process_group {
                    return Err(Error::InvalidStoredExecution {
                        execution: id,
                        reason: format!(
                            "the leader file names pid {pid}, the launched group is {process_group}"
                        ),
                    });
                }
                store::record_running(database, id, pid, &start).await?;
                return store::execution(database, id).await;
            }
            if launcher_exited.try_recv().is_ok() && read_leader_file(&leader_path).await?.is_none()
            {
                store::record_ended(database, id, EndReason::NeverStarted).await?;
                return Err(Error::LaunchFailed {
                    execution: id,
                    launcher_output: self.launcher_output(database, id).await,
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::LaunchTimedOut { execution: id });
            }
            tokio::time::sleep(OBSERVE_INTERVAL).await;
        }
    }

    async fn launcher_output(&self, database: &Database, id: ExecutionId) -> String {
        let path = self.output_path(database, id, OutputKind::Stderr);
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let quoted = &bytes[..bytes.len().min(LAUNCHER_OUTPUT_QUOTE_BYTES)];
                String::from_utf8_lossy(quoted).into_owned()
            }
            Err(error) => format!("(cannot read {}: {error})", path.display()),
        }
    }

    /// Looks at the real processes and records what it sees: a confirmed start, or an end.
    pub async fn observe(&self, database: &Database, id: ExecutionId) -> Result<Execution, Error> {
        let execution = store::execution(database, id).await?;
        self.record_presence(database, &execution).await?;
        store::execution(database, id).await
    }

    async fn record_presence(
        &self,
        database: &Database,
        execution: &Execution,
    ) -> Result<Presence, Error> {
        if !execution.phase.is_occupying() {
            return Ok(Presence::Gone(match execution.phase {
                Phase::Ended(reason) => reason,
                _ => unreachable!("not occupying means ended"),
            }));
        }
        let presence = self.presence(database, execution).await?;
        match &presence {
            Presence::Ours {
                confirmed: Some((pid, start)),
            } if execution.phase == Phase::Launching => {
                store::record_running(database, execution.id, *pid, start).await?;
            }
            Presence::Gone(reason) => {
                let reason = match reason {
                    EndReason::Exited if execution.cancel_requested => EndReason::Terminated,
                    reason => *reason,
                };
                store::record_ended(database, execution.id, reason).await?;
            }
            Presence::Ours { .. } | Presence::Unidentified => {}
        }
        Ok(presence)
    }

    async fn presence(
        &self,
        database: &Database,
        execution: &Execution,
    ) -> Result<Presence, Error> {
        if execution.boot_id != self.boot_id().await? {
            return Ok(Presence::Gone(EndReason::HostRestarted));
        }
        let leader_path = self.execution_dir(database, execution.id).join(LEADER_FILE);
        let leader_file = read_leader_file(&leader_path)
            .await?
            .filter(|(pid, _)| execution.process_group.is_none_or(|group| group == *pid));
        let started = execution.leader_start.is_some() || leader_file.is_some();
        let gone = Presence::Gone(if started {
            EndReason::Exited
        } else {
            EndReason::NeverStarted
        });
        if let Containment::SystemdScope { unit } = &execution.containment {
            if self.systemd.is_active(unit).await? {
                return Ok(Presence::Ours {
                    confirmed: leader_file,
                });
            }
            // Until `systemd-run` registered the scope, the unit reads as unknown, which
            // `systemctl` answers as `inactive`; the launcher's own group is still alive.
            let launcher_alive = execution
                .process_group
                .is_some_and(platform::group_has_members);
            return Ok(if !started && launcher_alive {
                Presence::Unidentified
            } else {
                gone
            });
        }
        let Some(process_group) = execution
            .process_group
            .or(leader_file.as_ref().map(|(pid, _)| *pid))
        else {
            return Ok(Presence::Unidentified);
        };
        if !platform::group_has_members(process_group) {
            return Ok(gone);
        }
        let expected_start = execution
            .leader_start
            .clone()
            .or(leader_file.as_ref().map(|(_, start)| start.clone()));
        let current_start = platform::leader_start(process_group).await?;
        Ok(match (expected_start, current_start) {
            // The leader exited and its group lives on. A group id is reused only after the
            // group emptied and a new process with that pid made itself leader, which the
            // leader-start check below catches while that new leader runs.
            (_, None) => Presence::Ours {
                confirmed: leader_file,
            },
            (Some(expected), Some(current)) if expected == current => Presence::Ours {
                confirmed: leader_file,
            },
            (Some(_), Some(_)) => gone,
            // A pid but no start token: the program has not confirmed its start, so a live
            // process with that pid cannot be told apart from a reused one.
            (None, Some(_)) => Presence::Unidentified,
        })
    }

    /// Records the cancel request, then `SIGTERM`, then `SIGKILL` after `grace`, to the whole
    /// containment. The termination counts only once the execution was observed empty;
    /// otherwise it keeps its reservation and the answer is [`Termination::Unconfirmed`].
    pub async fn terminate(
        &self,
        database: &Database,
        id: ExecutionId,
        escalation: Escalation,
    ) -> Result<Termination, Error> {
        store::record_cancel_requested(database, id).await?;
        let (execution, presence) = self.identify_for_termination(database, id).await?;
        let identity = match presence {
            Presence::Gone(_) => return Ok(Termination::Confirmed { after: None }),
            Presence::Unidentified => return Ok(Termination::Unconfirmed),
            Presence::Ours { confirmed } => SignalTarget::of(&execution, confirmed),
        };
        for (signal, wait) in [
            (Signal::Term, escalation.grace),
            (Signal::Kill, escalation.kill_wait),
        ] {
            self.signal(&identity, signal).await?;
            if self.wait_until_ended(database, id, wait).await? {
                return Ok(Termination::Confirmed {
                    after: Some(signal),
                });
            }
        }
        Ok(Termination::Unconfirmed)
    }

    /// The execution's presence for a stop request. One that is still launching gets up to
    /// the launch timeout to confirm its start, so a stop that races the launch is not lost.
    async fn identify_for_termination(
        &self,
        database: &Database,
        id: ExecutionId,
    ) -> Result<(Execution, Presence), Error> {
        let deadline = tokio::time::Instant::now() + self.launch_timeout;
        loop {
            let execution = store::execution(database, id).await?;
            let presence = self.record_presence(database, &execution).await?;
            let still_launching =
                matches!(presence, Presence::Unidentified) && execution.phase == Phase::Launching;
            if !still_launching || tokio::time::Instant::now() >= deadline {
                return Ok((execution, presence));
            }
            tokio::time::sleep(OBSERVE_INTERVAL).await;
        }
    }

    async fn signal(&self, target: &SignalTarget, signal: Signal) -> Result<(), Error> {
        match target {
            SignalTarget::Unit(unit) => self.systemd.signal(unit, signal).await,
            SignalTarget::ProcessGroup(group) => platform::signal_group(*group, signal),
        }
    }

    /// Observes until the execution ended or `wait` passed; true when it ended.
    async fn wait_until_ended(
        &self,
        database: &Database,
        id: ExecutionId,
        wait: Duration,
    ) -> Result<bool, Error> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if !self.observe(database, id).await?.phase.is_occupying() {
                return Ok(true);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(OBSERVE_INTERVAL).await;
        }
    }

    /// The restart contract's steps 2 and 3 (report 03): every execution that holds a
    /// reservation is reattached when positively identified, released when observed gone,
    /// and kept charged otherwise. One execution that cannot be checked (a slow
    /// `systemctl`, an unreadable leader file) stays charged as [`Reconciled::Unchecked`]
    /// instead of keeping the daemon from starting; only a database failure is an error.
    pub async fn reconcile(&self, database: &Database) -> Result<Vec<Reconciled>, Error> {
        let mut reconciled = Vec::new();
        for execution in store::occupying_executions(database).await? {
            let id = execution.id;
            reconciled.push(match self.record_presence(database, &execution).await {
                Ok(Presence::Ours { .. }) => Reconciled::Reattached(id),
                Ok(Presence::Gone(_)) => match store::execution(database, id).await?.phase {
                    Phase::Ended(reason) => Reconciled::Ended(id, reason),
                    _ => unreachable!("record_presence ended a gone execution"),
                },
                Ok(Presence::Unidentified) => Reconciled::Uncertain(id),
                Err(error @ Error::Database(_)) => return Err(error),
                Err(error) => Reconciled::Unchecked(id, error.to_string()),
            });
        }
        Ok(reconciled)
    }

    /// Follows one output file of an identified execution until it is observed empty, or
    /// until every clone of the stream is dropped.
    pub fn follow_output(
        &self,
        database: &Database,
        execution: &Execution,
        kind: OutputKind,
    ) -> Result<OutputStream, Error> {
        let identity = execution.identity().ok_or(Error::NotIdentified {
            execution: execution.id,
        })?;
        let supervisor = self.clone();
        let path = self.output_path(database, execution.id, kind);
        Ok(OutputStream::follow(path, self.output_limits, async move {
            while supervisor.identity_present(&identity).await {
                tokio::time::sleep(PRESENCE_POLL_INTERVAL).await;
            }
        }))
    }

    /// Whether processes of `identity` remain; a failing query counts as present, so output
    /// is never cut short by a flaky `systemctl`.
    async fn identity_present(&self, identity: &Identity) -> bool {
        match &identity.containment {
            Containment::SystemdScope { unit } => {
                self.systemd.is_active(unit).await.unwrap_or(true)
            }
            Containment::ProcessGroup => platform::group_has_members(identity.process_group),
        }
    }
}

enum SignalTarget {
    Unit(String),
    ProcessGroup(i32),
}

impl SignalTarget {
    fn of(execution: &Execution, confirmed: Option<(i32, String)>) -> SignalTarget {
        match &execution.containment {
            Containment::SystemdScope { unit } => SignalTarget::Unit(unit.clone()),
            Containment::ProcessGroup => SignalTarget::ProcessGroup(
                execution
                    .process_group
                    .or(confirmed.map(|(pid, _)| pid))
                    .expect("an identified process-group execution has its group"),
            ),
        }
    }
}

/// The launch confirmation the program wrote: its pid and start token. `None` until written;
/// the program writes it whole with a rename.
async fn read_leader_file(path: &Path) -> Result<Option<(i32, String)>, Error> {
    let contents = match tokio::fs::read_to_string(path).await {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::io("read", path)(error)),
    };
    let invalid = || Error::Io {
        action: "parse",
        path: path.to_path_buf(),
        source: std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("expected `<pid>\\n<start>\\n`, found {contents:?}"),
        ),
    };
    let mut lines = contents.lines();
    let pid = lines
        .next()
        .and_then(|line| line.parse::<i32>().ok())
        .ok_or_else(invalid)?;
    let start = lines
        .next()
        .map(str::trim)
        .filter(|start| !start.is_empty())
        .ok_or_else(invalid)?;
    Ok(Some((pid, start.to_owned())))
}
