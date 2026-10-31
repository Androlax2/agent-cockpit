//! Launch, observe, terminate and reconcile executions against the real processes.

use std::fs::{DirBuilder, OpenOptions};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use cockpit_turso::Database;
use tokio::sync::{OnceCell, oneshot};

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
    /// Positively identified. The row and the leader file merged: `process_group` is known
    /// for a process-group execution, `leader_start` once the program confirmed its start.
    Ours {
        process_group: Option<i32>,
        leader_start: Option<String>,
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
        let (id, containment) =
            store::record_intent(database, self.launch_containment, &boot_id).await?;
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
        let program = match containment {
            Containment::SystemdScope { .. } => self.systemd.systemd_run.clone(),
            Containment::ProcessGroup => PathBuf::from("/bin/sh"),
        };
        let mut command = tokio::process::Command::new(&program);
        if let Containment::SystemdScope { unit } = containment {
            command
                .args(["--user", "--scope", "--collect", "--quiet"])
                .arg(format!("--unit={unit}"))
                .args(["--", "/bin/sh"]);
        }
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

    /// Looks at the real processes, records a confirmed start or an end, and answers what
    /// it saw; a gone execution carries the end reason it was recorded with.
    async fn record_presence(
        &self,
        database: &Database,
        execution: &Execution,
    ) -> Result<Presence, Error> {
        if let Phase::Ended(reason) = execution.phase {
            return Ok(Presence::Gone(reason));
        }
        let presence = self.presence(database, execution).await?;
        match &presence {
            Presence::Ours {
                process_group: Some(group),
                leader_start: Some(start),
            } if execution.phase == Phase::Launching => {
                store::record_running(database, execution.id, *group, start).await?;
            }
            Presence::Gone(reason) => {
                let reason = match reason {
                    EndReason::Exited if execution.cancel_requested => EndReason::Terminated,
                    reason => *reason,
                };
                store::record_ended(database, execution.id, reason).await?;
                return Ok(Presence::Gone(reason));
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
        // Once running, the row holds everything the leader file says.
        let leader_file = if execution.phase == Phase::Launching {
            let leader_path = self.execution_dir(database, execution.id).join(LEADER_FILE);
            read_leader_file(&leader_path)
                .await?
                .filter(|(pid, _)| execution.process_group.is_none_or(|group| group == *pid))
        } else {
            None
        };
        let (file_group, file_start) = leader_file.unzip();
        let process_group = execution.process_group.or(file_group);
        let leader_start = execution.leader_start.clone().or(file_start);
        let gone = Presence::Gone(if leader_start.is_some() {
            EndReason::Exited
        } else {
            EndReason::NeverStarted
        });
        let ours = Presence::Ours {
            process_group,
            leader_start: leader_start.clone(),
        };
        if let Containment::SystemdScope { unit } = &execution.containment {
            if self.systemd.is_active(unit).await? {
                return Ok(ours);
            }
            // Until `systemd-run` registered the scope, the unit reads as unknown, which
            // `systemctl` answers as `inactive`; the launcher's own group is still alive.
            let launcher_alive = execution
                .process_group
                .is_some_and(platform::group_has_members);
            return Ok(if leader_start.is_none() && launcher_alive {
                Presence::Unidentified
            } else {
                gone
            });
        }
        let Some(process_group) = process_group else {
            return Ok(Presence::Unidentified);
        };
        if !platform::group_has_members(process_group) {
            return Ok(gone);
        }
        let current_start = platform::leader_start(process_group).await?;
        Ok(match (leader_start.as_deref(), current_start) {
            (Some(expected), Some(current)) if expected != current => gone,
            // A pid but no start token: the program has not confirmed its start, so a live
            // process with that pid cannot be told apart from a reused one.
            (None, Some(_)) => Presence::Unidentified,
            // The same leader, or a leader that exited while its group lives on. A group id
            // is reused only after the group emptied and a new process with that pid made
            // itself leader, which the start-token check above catches while it runs.
            _ => ours,
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
        let process_group = match presence {
            Presence::Gone(_) => return Ok(Termination::Confirmed { after: None }),
            Presence::Unidentified => return Ok(Termination::Unconfirmed),
            Presence::Ours { process_group, .. } => process_group,
        };
        for (signal, wait) in [
            (Signal::Term, escalation.grace),
            (Signal::Kill, escalation.kill_wait),
        ] {
            self.signal(&execution.containment, process_group, signal)
                .await?;
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

    async fn signal(
        &self,
        containment: &Containment,
        process_group: Option<i32>,
        signal: Signal,
    ) -> Result<(), Error> {
        match containment {
            Containment::SystemdScope { unit } => self.systemd.signal(unit, signal).await,
            Containment::ProcessGroup => platform::signal_group(
                process_group.expect("an identified process-group execution has its group"),
                signal,
            ),
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
            let execution = store::execution(database, id).await?;
            if let Presence::Gone(_) = self.record_presence(database, &execution).await? {
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
                Ok(Presence::Gone(reason)) => Reconciled::Ended(id, reason),
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
        let systemd = self.systemd.clone();
        let path = self.output_path(database, execution.id, kind);
        Ok(OutputStream::follow(path, self.output_limits, async move {
            while identity_present(&systemd, &identity).await {
                tokio::time::sleep(PRESENCE_POLL_INTERVAL).await;
            }
        }))
    }
}

/// Whether processes of `identity` remain; a failing query counts as present, so output is
/// never cut short by a flaky `systemctl`.
async fn identity_present(systemd: &SystemdTools, identity: &Identity) -> bool {
    match &identity.containment {
        Containment::SystemdScope { unit } => systemd.is_active(unit).await.unwrap_or(true),
        Containment::ProcessGroup => platform::group_has_members(identity.process_group),
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
