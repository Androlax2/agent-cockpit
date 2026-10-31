//! The systemd user manager as a containment: one transient scope per execution.

use std::path::PathBuf;

use crate::platform::host_command;
use crate::{Error, Signal};

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

    /// `systemd-run` set up to put the program it execs into `unit` before it runs.
    pub(crate) fn scope_launcher(&self, unit: &str) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.systemd_run);
        command
            .args(["--user", "--scope", "--collect", "--quiet"])
            .arg(format!("--unit={unit}"))
            .arg("--");
        command
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
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(program))
}

/// The scope of one execution. It embeds the database epoch and the row id, so a restart
/// finds the unit from the row alone and two state databases never share a unit.
pub(crate) fn scope_unit(epoch: &str, id: crate::ExecutionId) -> String {
    format!("cockpit-{epoch}-{}.scope", id.0)
}
