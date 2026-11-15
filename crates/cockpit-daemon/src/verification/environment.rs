//! Where a declared check runs (#352), as the owner's `agents.sandbox` decides (#862, Q1).
//! Under `"required"` a check runs through `torlek-sandbox`, still as an execution of the
//! daemon's supervisor; one that declares `native` is recorded `unavailable` and never runs on
//! the host. Under `"off"` a check of any environment but `docker` runs on the host as the
//! owner's approved commands run. A `docker` check runs its image in a container of the
//! project's Torlek-run engine (#856, #1392): its Docker client runs through `torlek-sandbox`,
//! which binds that engine's socket as a session's launch does, so it needs `"required"`.

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

const DOCKER_UNDER_OFF: Unplaced = Unplaced {
    reason: "docker checks run only with `agents.sandbox = \"required\"`: their Docker client \
        reaches the project's engine through the sandbox",
    next_action: "set `agents.sandbox = \"required\"`, or declare `environment = \"native\"`",
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
    /// In a container of the project's engine, its Docker client through `torlek-sandbox`.
    Docker,
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
        (ExecutionEnvironment::Docker, AgentSandbox::Off) => {
            Placement::Unavailable(DOCKER_UNDER_OFF)
        }
        (ExecutionEnvironment::Docker, AgentSandbox::Required) => Placement::Docker,
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

/// What a docker run records as where it ran (#1392): the project's engine, and the backend
/// its client ran in.
pub(super) fn ran_in_engine(
    backend: &cockpit_sandbox::NativeBackend,
    engine: &crate::containers::CheckEngine,
) -> Ran {
    Ran {
        environment: ExecutionEnvironment::Docker.as_str().to_owned(),
        isolation: format!(
            "project engine of {} (engine {}), its client in {}",
            engine.socket.user,
            engine.engine_id,
            backend.describe()
        ),
    }
}

/// The first executable `docker` in the directories of `path`, the daemon's `PATH`: the
/// client a docker check runs in the sandbox, as a session's agent runs it.
pub(super) fn docker_client(path: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path?)
        .map(|dir| dir.join("docker"))
        .find(|candidate| {
            candidate.metadata().is_ok_and(|metadata| {
                metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
            })
        })
}

/// The label naming the check run a docker run's container belongs to.
const RUN_LABEL: &str = "torlek.check-run";

/// What `docker run` exits with when the client or the engine failed (no image, a refused
/// pull, an engine gone), before the check's command ran.
const DOCKER_RUN_FAILED: i32 = 125;

/// The client's arguments that run `command` in `image` for check run `run` (#1392): labelled
/// with the run and removed when it ends, the checkout bound at its own path, so the engine
/// sees it through its masks, and started in `start_dir` with the check's shell whatever the
/// image's entry point is. An init process ends the command on the stop the client passes
/// on, so a stopped run's container does not outlive it. Nothing of the daemon's
/// environment, no credential and no CA enter the container (#856 Q5); the client's
/// configuration adds the engine's proxy. None for a checkout `--volume` cannot name.
pub(super) fn docker_arguments(
    run: &str,
    image: &str,
    command: &str,
    checkout: &Path,
    start_dir: &Path,
) -> Option<Vec<OsString>> {
    let checkout = checkout.to_str().filter(|path| !path.contains(':'))?;
    let mut arguments: Vec<OsString> = ["run", "--rm", "--init", "--label"]
        .map(OsString::from)
        .into();
    arguments.push(format!("{RUN_LABEL}={run}").into());
    arguments.push("--volume".into());
    arguments.push(format!("{checkout}:{checkout}").into());
    arguments.push("--workdir".into());
    arguments.push(start_dir.into());
    arguments.extend(["--entrypoint", super::CHECK_SHELL, image, "-c"].map(OsString::from));
    arguments.push(format!("exec 2>&1\n{command}").into());
    Some(arguments)
}

/// The outcome of a run that exited with `code`, not 0: a docker run whose client failed
/// before the command ran is `unavailable`, not a failure of the check.
pub(super) fn failed_outcome(ran: Option<&Ran>, code: i32) -> cockpit_verification::Outcome {
    let is_docker = ran.is_some_and(|ran| ran.environment == ExecutionEnvironment::Docker.as_str());
    if is_docker && code == DOCKER_RUN_FAILED {
        return cockpit_verification::Outcome::Unavailable;
    }
    cockpit_verification::Outcome::Failed
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
                Placement::Unavailable(DOCKER_UNDER_OFF),
            ),
            (AgentSandbox::Required, "docker", Placement::Docker),
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

#[cfg(test)]
mod docker_tests {
    use super::*;

    #[test]
    fn a_docker_check_runs_its_command_in_the_images_shell_in_the_checkout() {
        let arguments = docker_arguments(
            "execution-e-7",
            "rust:1.91",
            "cargo test",
            Path::new("/home/owner/app"),
            Path::new("/home/owner/app/crates"),
        )
        .unwrap();

        assert_eq!(
            arguments,
            [
                "run",
                "--rm",
                "--init",
                "--label",
                "torlek.check-run=execution-e-7",
                "--volume",
                "/home/owner/app:/home/owner/app",
                "--workdir",
                "/home/owner/app/crates",
                "--entrypoint",
                "/bin/sh",
                "rust:1.91",
                "-c",
                "exec 2>&1\ncargo test",
            ]
            .map(OsString::from)
        );
    }

    #[test]
    fn a_checkout_whose_path_holds_a_colon_is_not_bound() {
        let arguments = docker_arguments(
            "execution-e-7",
            "rust:1.91",
            "true",
            Path::new("/home/owner/a:b"),
            Path::new("/home/owner/a:b"),
        );

        assert_eq!(arguments, None);
    }

    #[test]
    fn a_docker_run_whose_client_failed_is_unavailable_not_failed() {
        use cockpit_verification::Outcome;
        let docker = Ran {
            environment: "docker".to_owned(),
            isolation: "project engine of torlek-e3".to_owned(),
        };
        let sandbox = Ran {
            environment: "sandbox".to_owned(),
            isolation: "torlek-sandbox".to_owned(),
        };

        for (case, ran, code, expected) in [
            ("client failed", Some(&docker), 125, Outcome::Unavailable),
            ("command failed", Some(&docker), 1, Outcome::Failed),
            ("sandboxed 125", Some(&sandbox), 125, Outcome::Failed),
            ("host 125", None, 125, Outcome::Failed),
        ] {
            assert_eq!(failed_outcome(ran, code), expected, "{case}");
        }
    }

    #[test]
    fn the_docker_client_is_the_first_executable_one_on_the_path() {
        use std::os::unix::fs::PermissionsExt;
        let folders = tempfile::tempdir().unwrap();
        let (plain, executable) = (folders.path().join("a"), folders.path().join("b"));
        for folder in [&plain, &executable] {
            std::fs::create_dir(folder).unwrap();
            std::fs::write(folder.join("docker"), "").unwrap();
        }
        let client = executable.join("docker");
        std::fs::set_permissions(&client, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([&plain, &executable]).unwrap();

        assert_eq!(docker_client(Some(&path)), Some(client));
        assert_eq!(docker_client(None), None);
    }
}
