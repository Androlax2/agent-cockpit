//! Where a declared check runs (#352), as the owner's `agents.sandbox` decides (#862, Q1).
//! Under `"required"` a check runs through `torlek-sandbox`, still as an execution of the
//! daemon's supervisor; one that declares `native` is recorded `unavailable` and never runs on
//! the host. Under `"off"` a check of any environment but `docker` runs on the host as the
//! owner's approved commands run. A `docker` check names its image, and is `unavailable` until
//! the project's engine runs it (#856, #1392).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use cockpit_config::{AgentSandbox, ExecutionEnvironment};
use cockpit_verification::Ran;

/// Where a check that runs nowhere stands: why, and what lifts it. Running it again does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Unplaced {
    pub(super) reason: &'static str,
    pub(super) next_action: &'static str,
}

const NATIVE_UNDER_REQUIRED: Unplaced = Unplaced {
    reason: "native checks run only with `agents.sandbox = \"off\"`",
    next_action: "declare `environment = \"sandbox\"`, or set `agents.sandbox = \"off\"`",
};

const DOCKER_NOT_RUN: Unplaced = Unplaced {
    reason: "docker checks run their image in a container of the project's Torlek-run engine, \
        which no run reaches before its checkout masks and container egress exist (#390)",
    next_action: "declare `environment = \"sandbox\"` until Torlek runs docker checks (#1392)",
};

const UNKNOWN_ENVIRONMENT: Unplaced = Unplaced {
    reason: "the check declares an environment Torlek does not know",
    next_action: "declare `environment = \"sandbox\"`",
};

/// How a host run is isolated: not at all, by the owner's own setting.
pub(super) const HOST_ISOLATION: &str = "no sandbox: agents.sandbox = \"off\"";

/// Where a check runs under the owner's setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Placement {
    /// On the host, as the owner's approved commands run.
    Host,
    /// Through `torlek-sandbox`.
    Sandbox,
    /// Nowhere: the receipt is `unavailable` with the reason, and no process starts.
    Unavailable(Unplaced),
}

/// Where a check that declares `declared` runs under `setting`.
pub(super) fn placement(setting: AgentSandbox, declared: &str) -> Placement {
    let Some(declared) = ExecutionEnvironment::ALL
        .into_iter()
        .find(|known| known.as_str() == declared)
    else {
        // The configuration refuses any other name; a check is never run on a guess.
        return Placement::Unavailable(UNKNOWN_ENVIRONMENT);
    };
    match (declared, setting) {
        (ExecutionEnvironment::Docker, _) => Placement::Unavailable(DOCKER_NOT_RUN),
        (_, AgentSandbox::Off) => Placement::Host,
        (ExecutionEnvironment::Native, AgentSandbox::Required) => {
            Placement::Unavailable(NATIVE_UNDER_REQUIRED)
        }
        (ExecutionEnvironment::Sandbox, AgentSandbox::Required) => Placement::Sandbox,
    }
}

/// What a host run records as where it ran.
pub(super) fn ran_on_host() -> Ran {
    Ran {
        environment: ExecutionEnvironment::Native.as_str().to_owned(),
        isolation: HOST_ISOLATION.to_owned(),
    }
}

/// What a sandboxed run records as where it ran: the backend the daemon probed.
pub(super) fn ran_in_sandbox(backend: &cockpit_sandbox::NativeBackend) -> Ran {
    Ran {
        environment: ExecutionEnvironment::Sandbox.as_str().to_owned(),
        isolation: backend.describe(),
    }
}

/// A sandboxed run's own folder in the daemon's folder of session temporary folders, beside
/// the sessions' own: the run's execution folder, with its `TMPDIR` and caches inside (#859).
/// Removed when the run ends.
pub(super) struct RunFolder {
    pub(super) root: PathBuf,
}

impl RunFolder {
    pub(super) fn new(session_temps: &Path, run: &str) -> RunFolder {
        RunFolder {
            root: session_temps.join(format!("check-{run}")),
        }
    }

    pub(super) fn temp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }
}

/// The environment Torlek builds for a sandboxed check: the daemon's own variables that
/// every agent starts with too, and the run's `TMPDIR`; none of the daemon's other variables
/// (its `GH_TOKEN` among them) enter it (#862, Q2).
pub(super) fn sandboxed_environment(
    daemon: &[(OsString, OsString)],
    temp_dir: &Path,
) -> Vec<(OsString, OsString)> {
    daemon
        .iter()
        .filter(|(name, _)| {
            crate::serve::AGENT_BASE_VARIABLES
                .iter()
                .any(|kept| name == kept)
        })
        .cloned()
        .chain([("TMPDIR".into(), temp_dir.into())])
        .collect()
}

/// The project's host grants (#674), as the decisions a check's proxy applies from its first
/// connection. A check serves no forward, so service grants are left out.
pub(super) fn host_grants(
    grants: Vec<cockpit_sandbox::NetworkGrant>,
) -> cockpit_sandbox::EgressDecisions {
    let allowed = grants
        .into_iter()
        .filter_map(|grant| match grant.target {
            cockpit_sandbox::NetworkTarget::Host { host, port } => {
                Some(cockpit_sandbox::AllowEntry {
                    target: cockpit_sandbox::Target::Name(host),
                    port,
                    source: cockpit_sandbox::AllowSource::Grant {
                        grant: grant.id.to_string(),
                    },
                })
            }
            cockpit_sandbox::NetworkTarget::Service { .. } => None,
        })
        .collect();
    cockpit_sandbox::EgressDecisions {
        allowed,
        ..cockpit_sandbox::EgressDecisions::default()
    }
}

/// The launcher in the supervisor's program place, its errors merged into the output as the
/// check's own are, so a receipt shows why a sandbox did not start.
pub(super) fn launcher_spec(
    prepared: cockpit_sandbox::Prepared,
    start_dir: PathBuf,
) -> cockpit_processes::LaunchSpec {
    let mut arguments: Vec<OsString> = vec![
        "-c".into(),
        "exec 2>&1\nexec \"$0\" \"$@\"".into(),
        prepared.program.into_os_string(),
    ];
    arguments.extend(prepared.arguments);
    cockpit_processes::LaunchSpec {
        program: super::CHECK_SHELL.into(),
        arguments,
        working_dir: start_dir,
        environment: prepared.environment,
        terminal: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_follows_the_owners_sandbox_setting() {
        for (setting, declared, expected) in [
            (AgentSandbox::Required, "sandbox", Placement::Sandbox),
            (
                AgentSandbox::Required,
                "native",
                Placement::Unavailable(NATIVE_UNDER_REQUIRED),
            ),
            (AgentSandbox::Off, "sandbox", Placement::Host),
            (AgentSandbox::Off, "native", Placement::Host),
            (
                AgentSandbox::Off,
                "docker",
                Placement::Unavailable(DOCKER_NOT_RUN),
            ),
            (
                AgentSandbox::Required,
                "docker",
                Placement::Unavailable(DOCKER_NOT_RUN),
            ),
        ] {
            assert_eq!(
                placement(setting, declared),
                expected,
                "{setting:?} {declared}"
            );
        }
    }

    #[test]
    fn an_unknown_environment_runs_nowhere() {
        assert_eq!(
            placement(AgentSandbox::Off, "vm"),
            Placement::Unavailable(UNKNOWN_ENVIRONMENT)
        );
    }

    #[test]
    fn a_sandboxed_check_gets_none_of_the_daemons_other_variables() {
        let daemon: Vec<(OsString, OsString)> = vec![
            ("HOME".into(), "/home/owner".into()),
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("GH_TOKEN".into(), "secret".into()),
            ("TMPDIR".into(), "/tmp".into()),
        ];

        let environment = sandboxed_environment(&daemon, Path::new("/state/tmp/check-r/tmp"));

        assert_eq!(
            environment,
            [
                (OsString::from("HOME"), OsString::from("/home/owner")),
                (OsString::from("PATH"), OsString::from("/usr/bin:/bin")),
                (
                    OsString::from("TMPDIR"),
                    OsString::from("/state/tmp/check-r/tmp")
                ),
            ]
        );
    }
}
