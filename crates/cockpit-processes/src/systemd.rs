//! The systemd user manager as a containment: one transient scope per execution.

use std::path::PathBuf;

use crate::platform::host_command;
use crate::{Error, Signal};

/// The first systemd whose `systemd-run` takes `--expand-environment`.
const MINIMUM_SYSTEMD: u32 = 254;

/// The programs that talk to this user's systemd manager. Paths, not lookups: the daemon
/// resolves them once and tests point them at fakes.
#[derive(Debug, Clone)]
pub struct SystemdTools {
    pub systemd_run: PathBuf,
    pub systemctl: PathBuf,
}

impl SystemdTools {
    /// `systemd-run` and `systemctl` resolved in the daemon's `PATH` now: an execution's
    /// environment is cleared, so a bare name would be looked up in the execution's `PATH`.
    /// A program not found keeps its bare name, and using it fails loud.
    pub fn from_path() -> SystemdTools {
        SystemdTools {
            systemd_run: resolve_in_path("systemd-run"),
            systemctl: resolve_in_path("systemctl"),
        }
    }

    /// Whether this user's manager answers. Without one (macOS, containers, a sandbox with
    /// no user bus) executions fall back to process groups, and each records which one held
    /// it.
    pub async fn manager_reachable(&self) -> bool {
        self.systemctl(&["--user", "show-environment"])
            .await
            .is_ok()
    }

    /// Whether `systemd-run` takes `--expand-environment=` (systemd 254 and later). Before
    /// 258 a scope never expanded `$`; from 258 it does unless told `no`.
    pub(crate) async fn takes_expand_environment(&self) -> bool {
        let Ok(printed) = host_command(&self.systemd_run.to_string_lossy(), &["--version"]).await
        else {
            return false;
        };
        printed
            .split_whitespace()
            .nth(1)
            .and_then(|version| version.parse::<u32>().ok())
            .is_some_and(|version| version >= MINIMUM_SYSTEMD)
    }

    /// `systemd-run` set up to put the program it execs into `unit` before it runs.
    pub(crate) fn scope_launcher(
        &self,
        unit: &str,
        takes_expand_environment: bool,
    ) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.systemd_run);
        command.args(Self::scope_arguments(unit, takes_expand_environment));
        command
    }

    /// [`SystemdTools::scope_launcher`] as words, for a host that runs the command itself.
    pub(crate) fn scope_launcher_words(
        &self,
        unit: &str,
        takes_expand_environment: bool,
    ) -> Vec<std::ffi::OsString> {
        let mut words = vec![self.systemd_run.clone().into_os_string()];
        words.extend(
            Self::scope_arguments(unit, takes_expand_environment)
                .into_iter()
                .map(std::ffi::OsString::from),
        );
        words
    }

    /// `--expand-environment=no` where `systemd-run` takes it: from systemd 258 a scope
    /// rewrites `$` in the command it runs (`$$` becomes `$`), which breaks the launch
    /// script.
    fn scope_arguments(unit: &str, takes_expand_environment: bool) -> Vec<String> {
        let mut arguments: Vec<String> = ["--user", "--scope", "--collect", "--quiet"]
            .map(str::to_owned)
            .into();
        if takes_expand_environment {
            arguments.push("--expand-environment=no".to_owned());
        }
        arguments.extend([format!("--unit={unit}"), "--".to_owned()]);
        arguments
    }

    /// Signals every process of the scope's cgroup, whatever process group it is in.
    pub(crate) async fn signal(&self, unit: &str, signal: Signal) -> Result<(), Error> {
        let name = match signal {
            Signal::Term => "--signal=SIGTERM",
            Signal::Kill => "--signal=SIGKILL",
        };
        match self
            .systemctl(&["--user", "kill", "--kill-who=all", name, unit])
            .await
        {
            Ok(_) => Ok(()),
            // A collected scope is already empty: what termination waits for.
            Err(_) if !self.is_active(unit).await? => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Whether the scope still has processes. A unit that emptied is collected, and an
    /// unknown unit reads as `inactive`.
    pub(crate) async fn is_active(&self, unit: &str) -> Result<bool, Error> {
        let state = self
            .systemctl(&["--user", "show", "--property=ActiveState", "--value", unit])
            .await?;
        Ok(matches!(
            state.trim(),
            "active" | "activating" | "deactivating" | "reloading" | "refreshing"
        ))
    }

    async fn systemctl(&self, arguments: &[&str]) -> Result<String, Error> {
        host_command(&self.systemctl.to_string_lossy(), arguments).await
    }
}

fn resolve_in_path(program: &str) -> PathBuf {
    find_in_path(program).unwrap_or_else(|| PathBuf::from(program))
}

/// The first file named `program` in the daemon's `PATH`.
pub fn find_in_path(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// The scope of one execution. It embeds the database epoch and the row id, so a restart
/// finds the unit from the row alone and two state databases never share a unit.
pub(crate) fn scope_unit(epoch: &str, id: crate::ExecutionId) -> String {
    format!("cockpit-{epoch}-{}.scope", id.0)
}
