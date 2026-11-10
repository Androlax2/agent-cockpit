//! The requirements one project's configuration adds (report 09): repository configuration
//! describes requirements, owner settings grant permissions.

use std::path::Path;

use cockpit_config::{AgentSandbox, EffectiveConfig, ExecutionEnvironment};

use crate::host::listed;
use crate::{
    CheckoutFacts, DockerEngine, EngineRemedy, EngineState, HostFacts, Platform, PresetFacts,
    ProjectEngine, ProjectFacts, ProviderFound, Requirement, Status, Trust,
};

/// Whether the project or one of its checks runs in Docker, so the daemon probes the engine.
pub fn needs_docker(config: &EffectiveConfig) -> bool {
    config.execution_environment.value == Some(ExecutionEnvironment::Docker)
        || config
            .checks
            .iter()
            .any(|check| check.value.environment == ExecutionEnvironment::Docker.as_str())
}

pub(crate) fn requirements(
    host: &HostFacts,
    config: &EffectiveConfig,
    project: &ProjectFacts,
) -> Vec<Requirement> {
    let mut requirements = vec![location(project, &host.platform)];
    requirements.extend(
        project
            .checkout
            .as_ref()
            .map(|checkout| trust(config, checkout)),
    );
    requirements.push(preset(config, &project.presets, host));
    requirements.extend(execution_environment(config, host));
    requirements.extend(needs(config, project));
    requirements.extend(checks(config, &project.dir));
    if let Some(docker) = &project.docker {
        requirements.push(self::docker(docker, &host.platform));
    }
    if let Some(engine) = &project.engine {
        requirements.push(project_engine(engine));
    }
    match &project.launch {
        Some(Ok(all)) => requirements.extend(crate::launch::launch_summary(all, &project.dir)),
        Some(Err(reason)) => requirements.push(Requirement::needs(
            "launch",
            Status::Unknown,
            format!("its gates could not be read: {reason}"),
            format!("`torlek sandbox show {}` names them", project.dir.display()),
        )),
        None => {}
    }
    requirements
}

fn location(project: &ProjectFacts, platform: &Platform) -> Requirement {
    let dir = project.dir.display();
    if let Platform::Wsl2 { windows_drives } = platform
        && let Some(drive) = windows_drives
            .iter()
            .find(|drive| project.dir.starts_with(drive))
    {
        return Requirement::needs(
            "project",
            Status::MissingPrerequisite,
            format!(
                "{dir} is on the Windows drive {}, which the sandbox denies to agents",
                drive.display()
            ),
            "move the project under your Linux home in WSL, then `torlek project add` it there",
        );
    }
    match &project.checkout {
        Some(checkout) => Requirement::available(
            "project",
            format!("{dir}, a checkout of {}", checkout.project),
        ),
        None => Requirement::needs(
            "project",
            Status::MissingPrerequisite,
            format!("{dir} is not a registered project"),
            format!("torlek project add {dir}"),
        ),
    }
}

fn trust(config: &EffectiveConfig, checkout: &CheckoutFacts) -> Requirement {
    let name = "project trust";
    match &checkout.trust {
        Trust::Trusted => Requirement::available(name, "trusted by the owner"),
        Trust::Inherited { from } => {
            Requirement::available(name, format!("trusted through {}", from.display()))
        }
        Trust::NeverForReview => Requirement::available(
            name,
            "a review clone: its own skills and guidance never load",
        ),
        Trust::Untrusted => Requirement::needs(
            name,
            Status::NeedsAuthorization,
            "untrusted: the project's own skills and guidance do not load",
            format!(
                "if you trust it, add \"{}\" to projects.trusted in {}",
                checkout.project_root.display(),
                config.user_file.path.display()
            ),
        ),
    }
}

/// The preset's agent steps need these capabilities from the provider that begins them.
fn preset(config: &EffectiveConfig, presets: &[PresetFacts], host: &HostFacts) -> Requirement {
    let Some(name) = &config.workflow_preset.value else {
        return Requirement::available(
            "workflow preset",
            "none set: a workflow takes the preset named when it starts",
        );
    };
    let source = &config.workflow_preset.source;
    let Some(preset) = presets.iter().find(|preset| &preset.name == name) else {
        let known: Vec<String> = presets.iter().map(|preset| preset.name.clone()).collect();
        return Requirement::needs(
            "workflow preset",
            Status::MissingPrerequisite,
            format!("{name:?} is not a preset; known: {}", listed(&known)),
            format!("fix workflow.preset at {source}"),
        );
    };
    let able: Vec<String> = host
        .providers
        .iter()
        .filter_map(|provider| {
            let found = provider.found.as_ref().ok()?;
            reports_all(found, &preset.capabilities).then(|| provider.name.clone())
        })
        .collect();
    let needed = listed(&preset.capabilities);
    if able.is_empty() {
        return Requirement::needs(
            "workflow preset",
            Status::MissingPrerequisite,
            format!("{name} needs a provider that reports {needed}; none available does"),
            "install a provider that does; `torlek provider list` shows what each reports",
        );
    }
    Requirement::available(
        "workflow preset",
        format!("{name} needs {needed}: {} report it", able.join(", ")),
    )
}

fn reports_all(found: &ProviderFound, capabilities: &[String]) -> bool {
    capabilities
        .iter()
        .all(|capability| found.capabilities.contains(capability))
}

/// Docker's own requirement carries a project that expects Docker.
fn execution_environment(config: &EffectiveConfig, host: &HostFacts) -> Option<Requirement> {
    let expected = config.execution_environment.value?;
    let source = &config.execution_environment.source;
    let name = "execution environment";
    let owner_file = config.user_file.path.display();
    if expected != ExecutionEnvironment::Docker
        && config.agents_sandbox.value != host.sandbox_setting
    {
        // The arms below would advise writing what the owner's file already says (#963).
        return Some(Requirement::needs(
            name,
            Status::Unknown,
            format!(
                "the project expects {} ({source}); what it meets depends on agents.sandbox = \
                 \"{}\", which the running daemon has not read",
                expected.as_str(),
                config.agents_sandbox.value.as_str()
            ),
            "apply the owner config as the `sandbox` line says, then run `torlek doctor` again",
        ));
    }
    if let (
        ExecutionEnvironment::Native | ExecutionEnvironment::Sandbox,
        AgentSandbox::Required,
        Ok(_),
        Some(refusal),
    ) = (
        expected,
        host.sandbox_setting,
        &host.sandbox_backend,
        &host.shared_desktop_refusal,
    ) {
        return Some(Requirement::needs(
            name,
            Status::MissingPrerequisite,
            format!(
                "the project expects {} ({source}); {refusal}; agents.sandbox = \"required\" refuses every launch",
                expected.as_str()
            ),
            "set agents.desktop_session = \"withheld\" in the owner config and run `torlek config reload`",
        ));
    }
    let requirement = match (expected, host.sandbox_setting) {
        (ExecutionEnvironment::Docker, _) => return None,
        (ExecutionEnvironment::Native, AgentSandbox::Off) => {
            Requirement::available(name, "native: the owner runs agents unsandboxed")
        }
        (ExecutionEnvironment::Native, AgentSandbox::Required) => match &host.sandbox_backend {
            Ok(backend) => Requirement::available(
                name,
                format!("native: agents run on this host, in {backend}"),
            ),
            Err(reason) => Requirement::needs(
                name,
                Status::MissingPrerequisite,
                format!(
                    "the project expects native work ({source}); agents.sandbox = \"required\" refuses every launch: {reason}"
                ),
                format!(
                    "fix what the detail names and restart the daemon, or, if you accept unsandboxed agents everywhere, set agents.sandbox = \"off\" in {owner_file}"
                ),
            ),
        },
        (ExecutionEnvironment::Sandbox, AgentSandbox::Required) => match &host.sandbox_backend {
            Ok(backend) => Requirement::available(
                name,
                format!("the project expects the sandbox ({source}); agents run in {backend}"),
            ),
            Err(reason) => Requirement::needs(
                name,
                Status::MissingPrerequisite,
                format!(
                    "the project expects the sandbox ({source}), and this host has none: {reason}"
                ),
                format!(
                    "fix what the detail names and restart the daemon, or set agents.sandbox = \"off\" in {owner_file} to run its agents unsandboxed"
                ),
            ),
        },
        (ExecutionEnvironment::Sandbox, AgentSandbox::Off) => Requirement::needs(
            name,
            Status::NeedsAuthorization,
            format!(
                "the project expects the sandbox ({source}); agents run unsandboxed: agents.sandbox = \"off\""
            ),
            format!(
                "set agents.sandbox = \"required\" in {owner_file} to run its agents in Torlek's sandbox, or start them knowing they run unsandboxed"
            ),
        ),
    };
    Some(requirement)
}

fn needs(config: &EffectiveConfig, project: &ProjectFacts) -> Option<Requirement> {
    let needs = config.needs.value?;
    let capacity = project.capacity;
    let detail = format!(
        "{} CPU millis and {} MiB; this host's budget is {} CPU millis and {} MiB",
        needs.cpu_millis, needs.memory_mib, capacity.cpu_millis, capacity.memory_mib
    );
    if needs.cpu_millis <= capacity.cpu_millis && needs.memory_mib <= capacity.memory_mib {
        return Some(Requirement::available("needs", detail));
    }
    Some(Requirement::needs(
        "needs",
        Status::Unsupported,
        format!("{detail}, so its work is never admitted"),
        format!(
            "lower [needs] at {}, or work on the project from a larger host",
            config.needs.source
        ),
    ))
}

fn checks(config: &EffectiveConfig, dir: &Path) -> Vec<Requirement> {
    config
        .checks
        .iter()
        .map(|check| {
            let name = format!("check {}", check.value.name);
            let working_directory = &check.value.working_directory;
            if dir.join(working_directory).is_dir() {
                return Requirement::available(
                    name,
                    format!(
                        "`{}` in {working_directory} ({})",
                        check.value.command, check.value.environment
                    ),
                );
            }
            Requirement::needs(
                name,
                Status::MissingPrerequisite,
                format!("its working directory {working_directory} is not a folder of the project"),
                format!("create it, or fix the check at {}", check.source),
            )
        })
        .collect()
}

fn docker(docker: &Result<DockerEngine, String>, platform: &Platform) -> Requirement {
    let engine = match docker {
        Ok(engine) => engine,
        Err(reason) => {
            return Requirement::needs(
                "docker",
                Status::MissingPrerequisite,
                reason.clone(),
                "install and start a Docker engine for this user; a docker program alone is not an engine",
            );
        }
    };
    let identity = identity(engine);
    if engine.rootless {
        return Requirement::available("docker", format!("{identity}, rootless"));
    }
    // macOS runs no Linux containers itself: every engine there lives in a VM.
    if *platform == Platform::MacOs {
        return Requirement::available("docker", format!("{identity}, inside its Linux VM"));
    }
    if engine.in_desktop_vm {
        return Requirement::available("docker", format!("{identity}, inside Docker Desktop's VM"));
    }
    Requirement::needs(
        "docker",
        Status::MissingPrerequisite,
        format!("{identity} is rootful: whoever reaches its socket is root on this host"),
        "set up rootless Docker for this user (https://docs.docker.com/engine/security/rootless/), set DOCKER_HOST to its socket in the daemon's environment, then restart the daemon",
    )
}

/// The project's own engine under `agents.sandbox = "required"` (#390, #856's table). A
/// verified engine is still `unsupported`: sandboxed sessions do not reach it yet.
fn project_engine(engine: &Result<ProjectEngine, String>) -> Requirement {
    let engine = match engine {
        Ok(engine) => engine,
        Err(reason) => {
            return Requirement::needs(
                "docker",
                Status::Unknown,
                format!("the project's engine could not be examined: {reason}"),
                "fix what it names, then run `torlek doctor` again",
            );
        }
    };
    let ProjectEngine {
        user,
        record_command,
        state,
        ..
    } = engine;
    let record = format!("`{record_command}`");
    match state {
        EngineState::NotSetUp { reason, setup } => Requirement::needs(
            "docker",
            Status::MissingPrerequisite,
            format!(
                "no Torlek-run engine for this project ({reason}); sandboxed sessions never \
                 reach your own engine"
            ),
            match setup {
                Ok(script) => format!(
                    "review {}, run `sudo sh {}`, then {record}",
                    script.display(),
                    script.display()
                ),
                Err(reason) => format!("the setup script could not be written: {reason}"),
            },
        ),
        EngineState::Unrecorded(found) => Requirement::needs(
            "docker",
            Status::MissingPrerequisite,
            format!(
                "{} answers as {user}, rootless, and is not recorded",
                identity(found)
            ),
            format!("if you set it up, {record} records it"),
        ),
        EngineState::Refused { reason, remedy } => Requirement::needs(
            "docker",
            Status::MissingPrerequisite,
            format!("the engine of {user} is refused: {reason}"),
            match remedy {
                EngineRemedy::Setup(Ok(script)) => format!(
                    "review {}, run `sudo sh {}` again, then {record}",
                    script.display(),
                    script.display()
                ),
                EngineRemedy::Setup(Err(reason)) => {
                    format!("the setup script could not be written: {reason}")
                }
                EngineRemedy::Start { unit } => format!(
                    "`sudo systemctl start {unit}`; `journalctl -u {unit}` says why it stopped"
                ),
                EngineRemedy::Record => {
                    format!("if you set up the engine that answers now, {record} records it")
                }
            },
        ),
        EngineState::Verified(found) => Requirement::needs(
            "docker",
            Status::Unsupported,
            format!(
                "{} runs as {user}, rootless, recorded; sandboxed sessions do not reach it yet \
                 in this release (#390)",
                identity(found)
            ),
            "`torlek ask run` asks you to run a container command on the host",
        ),
    }
}

fn identity(engine: &DockerEngine) -> String {
    format!(
        "engine {} on {} (Docker {})",
        engine.id, engine.name, engine.server_version
    )
}
