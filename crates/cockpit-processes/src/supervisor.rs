//! Launch, observe, terminate and reconcile executions against the real processes.

use std::ffi::{OsStr, OsString};
use std::fs::{DirBuilder, OpenOptions};
use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use cockpit_turso::{Database, WriteTransaction};
use tokio::sync::{OnceCell, oneshot};

use crate::output::{OutputKind, OutputLimits, OutputStream};
use crate::platform::{self, LAUNCH_SCRIPT};
use crate::systemd::SystemdTools;
use crate::{
    Containment, EndReason, Error, Escalation, Execution, ExecutionId, HostedLaunch, Identity,
    LaunchSpec, Phase, Reconciled, Signal, Termination, store,
};

const LEADER_FILE: &str = "leader";
/// A hosted launch's environment, read and deleted by its host step before the program runs.
const ENVIRONMENT_FILE: &str = "environment";
/// How often a wait for a launch confirmation or for an empty execution looks again.
const OBSERVE_INTERVAL: Duration = Duration::from_millis(20);
/// How often a followed stream checks whether its execution emptied. Slower than
/// [`OBSERVE_INTERVAL`]: for a scope each check spawns `systemctl`.
const PRESENCE_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// The launcher's stderr quoted in [`Error::LaunchFailed`].
const LAUNCHER_OUTPUT_QUOTE_BYTES: usize = 2048;
/// How often [`Supervisor::watch_until_ended`] looks: first after `WATCH_FIRST`, then twice
/// as long each time up to `WATCH_LONGEST`. Each look at a scope runs `systemctl`, and a
/// watched execution may run for days.
const WATCH_FIRST: Duration = Duration::from_secs(1);
const WATCH_LONGEST: Duration = Duration::from_secs(30);

type LauncherExit = std::io::Result<std::process::ExitStatus>;

/// A launched execution, and its program's exit status: the launcher execs the program
/// (`systemd-run --scope` and the session leader too), so the status it reaps is the
/// program's own. A launcher reaped by init after a daemon restart has none to report.
#[derive(Debug)]
pub struct Launched {
    pub execution: Execution,
    pub exit: oneshot::Receiver<LauncherExit>,
}

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
    /// Asked once, at the first scope launch.
    takes_expand_environment: Arc<OnceCell<bool>>,
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
            takes_expand_environment: Arc::new(OnceCell::new()),
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

    /// Whether a launch in `containment` passes `--expand-environment=no`: a scope whose
    /// `systemd-run` knows the switch. Asked once.
    async fn scope_takes_expand_environment(&self, containment: &Containment) -> bool {
        if !matches!(containment, Containment::SystemdScope { .. }) {
            return false;
        }
        *self
            .takes_expand_environment
            .get_or_init(|| self.systemd.takes_expand_environment())
            .await
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
        let write = database.write().await?;
        let id = self.reserve(&write, database.epoch()).await?;
        write.commit().await?;
        Ok(self.launch_reserved(database, id, spec).await?.execution)
    }

    /// Records a launch intent in the caller's transaction, so a capability can commit it
    /// with its own record of what the execution is for (report 02, cross-capability
    /// atomicity). `epoch` is the database's. Once committed, launch it with
    /// [`Supervisor::launch_reserved`]; until then, or if it never is, a restart's
    /// reconciliation ends it as never started.
    pub async fn reserve(
        &self,
        write: &WriteTransaction<'_>,
        epoch: &str,
    ) -> Result<ExecutionId, Error> {
        let boot_id = self.boot_id().await?;
        store::record_intent(write, epoch, self.launch_containment, boot_id).await
    }

    /// Spawns the program of the committed intent `id` in its containment and waits for its
    /// start confirmation, as [`Supervisor::launch`] does.
    pub async fn launch_reserved(
        &self,
        database: &Database,
        id: ExecutionId,
        spec: &LaunchSpec,
    ) -> Result<Launched, Error> {
        let containment = reserved_execution(database, id).await?.containment;
        let takes_expand_environment = self.scope_takes_expand_environment(&containment).await;
        let mut launcher = match self.spawn(
            self.execution_dir(database, id),
            &containment,
            takes_expand_environment,
            spec,
        ) {
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
        // The daemon reaps its own children, so an exited leader never lingers as a zombie
        // that keeps its process group alive. After a restart, init reaps them.
        let (exited_sender, launcher_exited) = oneshot::channel();
        tokio::spawn(async move {
            let _ = exited_sender.send(launcher.wait().await);
        });
        store::record_process_group(database, id, process_group).await?;
        self.confirm_start(
            database,
            id,
            process_group,
            launcher_exited,
            spec.terminal.is_some(),
        )
        .await
    }

    fn spawn(
        &self,
        dir: PathBuf,
        containment: &Containment,
        takes_expand_environment: bool,
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
        // A terminal execution's session leader comes first, so the launch script and the
        // program already run in its session and process group.
        let mut launcher: Vec<&OsStr> = Vec::new();
        if let Some(terminal) = &spec.terminal {
            launcher.push(terminal.leader.as_os_str());
            launcher.extend(terminal.leader_arguments.iter().map(OsString::as_os_str));
        }
        launcher.push(OsStr::new("/bin/sh"));
        let (mut command, program) = match containment {
            Containment::SystemdScope { unit } => {
                let mut command = self.systemd.scope_launcher(unit, takes_expand_environment);
                command.args(&launcher);
                (command, self.systemd.systemd_run.clone())
            }
            Containment::ProcessGroup => {
                let mut command = tokio::process::Command::new(launcher[0]);
                command.args(&launcher[1..]);
                (command, PathBuf::from(launcher[0]))
            }
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
            .envs(spec.environment.iter().map(|(name, value)| (name, value)));
        // A terminal execution's output, the launcher's errors included, goes to its terminal:
        // it has no output files.
        match &spec.terminal {
            Some(terminal) => {
                let device = |device: &OwnedFd| {
                    device
                        .try_clone()
                        .map_err(Error::io("duplicate", "the execution's terminal"))
                };
                // No process group of its own here: `setsid` refuses a group leader, and
                // the session leader's new session is the execution's group.
                command
                    .stdin(device(&terminal.device)?)
                    .stdout(device(&terminal.device)?)
                    .stderr(device(&terminal.device)?);
            }
            None => {
                command
                    .stdin(Stdio::null())
                    .stdout(output_file(OutputKind::Stdout)?)
                    .stderr(output_file(OutputKind::Stderr)?)
                    .process_group(0);
            }
        }
        if let Containment::SystemdScope { .. } = containment {
            command.envs(scope_manager_variables(&spec.environment));
        }
        command.spawn().map_err(Error::io("spawn", program))
    }

    /// Commits the launch intent of an execution whose program a host starts itself: a tmux
    /// pane, whose terminal the program needs (report 05). The host runs the returned
    /// command in `spec.working_dir` as the leader of a new process group (tmux makes each
    /// pane's process one); [`Supervisor::confirm_hosted`] then waits for its start, or
    /// [`Supervisor::abandon_hosted`] releases the reservation when the host could not start
    /// it. The command clears the host's environment, so the program gets exactly
    /// `spec.environment` plus the `host_variables` the host has set: those describe the
    /// host itself, as `TERM` names the terminal tmux emulates. Output goes to the host's
    /// terminal, not to the output files.
    pub async fn prepare_hosted(
        &self,
        database: &Database,
        spec: &LaunchSpec,
        host_variables: &[&str],
    ) -> Result<HostedLaunch, Error> {
        let environment = self.launch_environment(spec)?;
        let host_script = host_script(host_variables, &environment)?;
        let write = database.write().await?;
        let id = self.reserve(&write, database.epoch()).await?;
        write.commit().await?;
        let containment = store::execution(database, id).await?.containment;
        self.prepare_host_command(database, id, &containment, spec, &environment, &host_script)
            .await
    }

    /// [`Supervisor::prepare_hosted`] for the committed intent `id`, which the caller
    /// reserved with [`Supervisor::reserve`] in the transaction that records what the
    /// execution is for: a crash between the two commits would otherwise leave a reserved
    /// execution nobody names (#256). A refused `spec` ends the intent as never started.
    pub async fn prepare_reserved_hosted(
        &self,
        database: &Database,
        id: ExecutionId,
        spec: &LaunchSpec,
        host_variables: &[&str],
    ) -> Result<HostedLaunch, Error> {
        let containment = reserved_execution(database, id).await?.containment;
        let prepared = self.launch_environment(spec).and_then(|environment| {
            let host_script = host_script(host_variables, &environment)?;
            Ok((environment, host_script))
        });
        let (environment, host_script) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                store::record_ended(database, id, EndReason::NeverStarted).await?;
                return Err(error);
            }
        };
        self.prepare_host_command(database, id, &containment, spec, &environment, &host_script)
            .await
    }

    /// The host's command for the reserved execution `id`: its private directory and
    /// environment file, then the words that run `spec` in its containment.
    async fn prepare_host_command(
        &self,
        database: &Database,
        id: ExecutionId,
        containment: &Containment,
        spec: &LaunchSpec,
        environment: &[(OsString, OsString)],
        host_script: &str,
    ) -> Result<HostedLaunch, Error> {
        let dir = self.execution_dir(database, id);
        if let Err(error) = DirBuilder::new().recursive(true).mode(0o700).create(&dir) {
            store::record_ended(database, id, EndReason::NeverStarted).await?;
            return Err(Error::io("create", dir)(error));
        }
        // Values travel in a private file, never in argv, where any local process could
        // read them: the launch token among them.
        let environment_file = dir.join(ENVIRONMENT_FILE);
        if let Err(error) = write_environment_file(&environment_file, environment) {
            store::record_ended(database, id, EndReason::NeverStarted).await?;
            return Err(Error::io("write", environment_file)(error));
        }
        let mut command = ["/bin/sh", "-c", host_script, "cockpit-host"]
            .map(OsString::from)
            .to_vec();
        command.push(environment_file.into_os_string());
        if let Containment::SystemdScope { unit } = containment {
            let takes_expand_environment = self.scope_takes_expand_environment(containment).await;
            command.extend(
                self.systemd
                    .scope_launcher_words(unit, takes_expand_environment),
            );
        }
        command.extend(["/bin/sh", "-c", LAUNCH_SCRIPT, "cockpit-launch"].map(OsString::from));
        command.push(dir.join(LEADER_FILE).into_os_string());
        command.push(spec.program.clone().into_os_string());
        command.extend(spec.arguments.iter().cloned());
        Ok(HostedLaunch {
            execution: id,
            command,
        })
    }

    /// The environment a hosted program gets: `spec.environment`, plus what `systemd-run`
    /// needs to reach the manager when the execution runs in a scope. Refuses a name that is
    /// not a plain shell name, and a value holding a NUL byte.
    fn launch_environment(&self, spec: &LaunchSpec) -> Result<Vec<(OsString, OsString)>, Error> {
        let mut environment = spec.environment.clone();
        if let Some((name, _)) = environment.iter().find(|(name, value)| {
            !name.to_str().is_some_and(is_shell_name) || value.as_encoded_bytes().contains(&0)
        }) {
            return Err(Error::InvalidEnvironment {
                name: name.to_string_lossy().into_owned(),
            });
        }
        if self.launch_containment == LaunchContainment::SystemdScope {
            let manager = scope_manager_variables(&environment);
            environment.extend(manager);
        }
        Ok(environment)
    }

    /// Waits for a hosted execution's start confirmation, the leader file its command
    /// writes; the leader's pid is its process group. On [`Error::LaunchTimedOut`] the
    /// execution keeps its reservation, as for [`Supervisor::launch`].
    pub async fn confirm_hosted(
        &self,
        database: &Database,
        id: ExecutionId,
    ) -> Result<Execution, Error> {
        let leader_path = self.execution_dir(database, id).join(LEADER_FILE);
        let deadline = tokio::time::Instant::now() + self.launch_timeout;
        loop {
            if let Some((pid, start)) = read_leader_file(&leader_path).await? {
                store::record_running(database, id, pid, &start).await?;
                return store::execution(database, id).await;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::LaunchTimedOut { execution: id });
            }
            tokio::time::sleep(OBSERVE_INTERVAL).await;
        }
    }

    /// Releases a hosted execution's reservation when its host never ran the command: no
    /// leader file exists, so no process of it can. A command that did start is left to
    /// [`Supervisor::observe`].
    pub async fn abandon_hosted(&self, database: &Database, id: ExecutionId) -> Result<(), Error> {
        let dir = self.execution_dir(database, id);
        if read_leader_file(&dir.join(LEADER_FILE)).await?.is_some() {
            return Ok(());
        }
        match tokio::fs::remove_file(dir.join(ENVIRONMENT_FILE)).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(Error::io("remove", dir.join(ENVIRONMENT_FILE))(error)),
        }
        store::record_ended(database, id, EndReason::NeverStarted).await?;
        Ok(())
    }

    async fn confirm_start(
        &self,
        database: &Database,
        id: ExecutionId,
        process_group: i32,
        mut launcher_exited: oneshot::Receiver<LauncherExit>,
        has_terminal: bool,
    ) -> Result<Launched, Error> {
        let leader_path = self.execution_dir(database, id).join(LEADER_FILE);
        let deadline = tokio::time::Instant::now() + self.launch_timeout;
        // Kept once received: a program that exits at once still has its status reported.
        let mut exited = None;
        loop {
            // Looked at before the leader file: an exit seen first and no file after it means
            // the program never started, since the file is written before the program runs.
            if exited.is_none() {
                exited = launcher_exited.try_recv().ok();
            }
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
                let exit = match exited {
                    Some(status) => {
                        let (sender, exit) = oneshot::channel();
                        let _ = sender.send(status);
                        exit
                    }
                    None => launcher_exited,
                };
                return Ok(Launched {
                    execution: store::execution(database, id).await?,
                    exit,
                });
            }
            if exited.is_some() {
                store::record_ended(database, id, EndReason::NeverStarted).await?;
                return Err(Error::LaunchFailed {
                    execution: id,
                    launcher_output: if has_terminal {
                        "(written to its terminal)".to_owned()
                    } else {
                        self.launcher_output(database, id).await
                    },
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

    /// Observes `id` until it ended, however long that takes, and answers why it ended. A
    /// host query that failed (`systemctl`, `/proc`) is asked again at the next look; only the
    /// state database or the stored row failing ends the watch.
    pub async fn watch_until_ended(
        &self,
        database: &Database,
        id: ExecutionId,
    ) -> Result<EndReason, Error> {
        let mut interval = WATCH_FIRST;
        loop {
            match self.observe(database, id).await {
                Ok(Execution {
                    phase: Phase::Ended(reason),
                    ..
                }) => return Ok(reason),
                Ok(_) | Err(Error::HostCommand { .. } | Error::Io { .. }) => {}
                Err(error) => return Err(error),
            }
            tokio::time::sleep(interval).await;
            interval = (interval * 2).min(WATCH_LONGEST);
        }
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
                let recorded = store::record_ended(database, execution.id, reason).await?;
                return Ok(Presence::Gone(recorded));
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
        // Once running, the row holds everything the leader file says: the table's CHECK
        // refuses a running row without its process group and start token.
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
            // `systemctl` answers as `inactive`; the launcher itself is still alive.
            return Ok(launching_or(gone, &leader_start, execution.process_group));
        }
        let Some(process_group) = process_group else {
            return Ok(Presence::Unidentified);
        };
        if !platform::group_has_members(process_group) {
            return Ok(launching_or(gone, &leader_start, Some(process_group)));
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

/// `gone`, unless the execution has not confirmed its start and its launcher (whose pid names
/// the group) still runs. A terminal execution's launcher leads that group only once it is
/// the session leader: until then its pid is alive in the daemon's group and the group has
/// no member yet.
/// The execution `id`, refused unless it is a launch intent still waiting for its launch.
async fn reserved_execution(database: &Database, id: ExecutionId) -> Result<Execution, Error> {
    let reserved = store::execution(database, id).await?;
    if reserved.phase != Phase::Launching || reserved.process_group.is_some() {
        return Err(Error::NotReserved { execution: id });
    }
    Ok(reserved)
}

fn launching_or(gone: Presence, leader_start: &Option<String>, launcher: Option<i32>) -> Presence {
    let is_launcher_alive = launcher
        .is_some_and(|pid| platform::group_has_members(pid) || platform::process_exists(pid));
    if leader_start.is_none() && is_launcher_alive {
        Presence::Unidentified
    } else {
        gone
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

/// The daemon's variables `systemd-run --user` finds this user's manager through, for those
/// `environment` does not set: with `--scope` it execs the program, which inherits them, and
/// a spec's own value wins.
fn scope_manager_variables(environment: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
    ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"]
        .into_iter()
        .filter(|name| !environment.iter().any(|(set, _)| set == name))
        .filter_map(|name| Some((name.into(), std::env::var_os(name)?)))
        .collect()
}

/// The host's shell step of a hosted command. It reads the environment file (its first
/// argument) and deletes it, then runs `env -i` with each of `host_names` the host has set and
/// each name of `environment`, then the words after the file. Names are refused unless they
/// are plain shell names, since they are written into the script.
fn host_script(host_names: &[&str], environment: &[(OsString, OsString)]) -> Result<String, Error> {
    let mut script =
        String::from("set -e; . \"$1\"; /bin/rm -f -- \"$1\"; shift; set +e; exec /usr/bin/env -i");
    for name in host_names {
        if !is_shell_name(name) {
            return Err(Error::InvalidEnvironment {
                name: (*name).to_owned(),
            });
        }
        script.push_str(&format!(" ${{{name}+\"{name}=${name}\"}}"));
    }
    for (name, _) in environment {
        // launch_environment admitted only shell names.
        let name = name.to_string_lossy();
        script.push_str(&format!(" \"{name}=${name}\""));
    }
    script.push_str(" \"$@\"");
    Ok(script)
}

fn is_shell_name(name: &str) -> bool {
    let mut characters = name.chars();
    characters
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

/// Writes `environment` as shell assignments, each value single-quoted, readable by this
/// user only.
fn write_environment_file(
    path: &Path,
    environment: &[(OsString, OsString)],
) -> std::io::Result<()> {
    let mut contents = Vec::new();
    for (name, value) in environment {
        contents.extend_from_slice(name.as_encoded_bytes());
        contents.extend_from_slice(b"='");
        for &byte in value.as_encoded_bytes() {
            match byte {
                b'\'' => contents.extend_from_slice(b"'\\''"),
                byte => contents.push(byte),
            }
        }
        contents.extend_from_slice(b"'\n");
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?
        .write_all(&contents)
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
