//! The requirements one project's configuration adds (report 09): repository configuration
//! describes requirements, owner settings grant permissions.

use std::path::Path;

use cockpit_config::{AgentSandbox, EffectiveConfig, ExecutionEnvironment};

use crate::host::listed;
use crate::{
    CheckoutFacts, DockerEngine, HostFacts, Platform, PresetFacts, ProjectFacts, ProviderFound,
    Requirement, Status, Trust,
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
            "move the project under your Linux home in WSL, then `cockpit project add` it there",
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
            format!("cockpit project add {dir}"),
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
            "install a provider that does; `cockpit provider list` shows what each reports",
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
    let requirement = match (expected, host.sandbox_setting) {
        (ExecutionEnvironment::Docker, _) => return None,
        (ExecutionEnvironment::Native, AgentSandbox::Off) => {
            Requirement::available(name, "native: the owner runs agents unsandboxed")
        }
        (ExecutionEnvironment::Native, AgentSandbox::Required) => Requirement::needs(
            name,
            Status::NeedsAuthorization,
            format!("the project expects native work ({source}); agents run sandboxed"),
            format!(
                "keep it sandboxed, or, if you accept unsandboxed agents everywhere, set agents.sandbox = \"off\" in {owner_file}"
            ),
        ),
        (ExecutionEnvironment::Sandbox, AgentSandbox::Required) => match &host.sandbox_engine {
            Ok(_) => Requirement::available(name, "sandbox"),
            Err(_) => Requirement::needs(
                name,
                Status::MissingPrerequisite,
                "the project expects the sandbox, and this host has no engine",
                "make the `sandbox` requirement available",
            ),
        },
        (ExecutionEnvironment::Sandbox, AgentSandbox::Off) => Requirement::needs(
            name,
            Status::NeedsAuthorization,
            format!(
                "the project expects the sandbox ({source}); the owner runs agents unsandboxed"
            ),
            format!(
                "remove agents.sandbox = \"off\" from {owner_file} and run `cockpit config reload`"
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
    let identity = format!(
        "engine {} on {} (Docker {})",
        engine.id, engine.name, engine.server_version
    );
    if engine.rootless {
        return Requirement::available("docker", format!("{identity}, rootless"));
    }
    if *platform == Platform::MacOs {
        return Requirement::available("docker", format!("{identity}, inside its Linux VM"));
    }
    Requirement::needs(
        "docker",
        Status::MissingPrerequisite,
        format!("{identity} is rootful: whoever reaches its socket is root on this host"),
        "set up rootless Docker for this user (https://docs.docker.com/engine/security/rootless/) and point DOCKER_HOST at it",
    )
}
