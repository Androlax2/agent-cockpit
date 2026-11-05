//! The requirements every host has, project or not.

use cockpit_config::{AgentSandbox, ConfigFile, EffectiveConfig, KNOWN_PROVIDERS};

use crate::{ControlFacts, Platform, ProviderFacts, ProviderUnusable, Requirement, Status};

const RESTART: &str = "then restart the daemon";

pub(crate) fn config(config: &Result<EffectiveConfig, String>) -> Requirement {
    match config {
        Ok(config) => {
            let mut files = vec![file("owner", &config.user_file)];
            files.extend(
                config
                    .project_file
                    .as_ref()
                    .map(|project| file("project", project)),
            );
            Requirement::available("config", files.join(", "))
        }
        Err(error) => Requirement::needs(
            "config",
            Status::MissingPrerequisite,
            error.clone(),
            "fix the file and line named here; `torlek config show` reads it again",
        ),
    }
}

fn file(role: &str, file: &ConfigFile) -> String {
    let presence = if file.is_present { "read" } else { "absent" };
    format!("{role} {} ({presence})", file.path.display())
}

pub(crate) fn platform(platform: &Platform) -> Requirement {
    match platform {
        Platform::Linux => Requirement::available("platform", "linux"),
        Platform::MacOs => Requirement::available("platform", "macos"),
        Platform::Wsl2 { windows_drives } => {
            let drives: Vec<String> = windows_drives
                .iter()
                .map(|drive| drive.display().to_string())
                .collect();
            let detail = if drives.is_empty() {
                "wsl2".to_owned()
            } else {
                format!(
                    "wsl2; agents never reach the Windows drives {}",
                    drives.join(", ")
                )
            };
            Requirement::available("platform", detail)
        }
        Platform::Unsupported(name) => Requirement::needs(
            "platform",
            Status::Unsupported,
            format!("{name}: Torlek runs on Linux and macOS, and on Windows inside WSL2"),
            "run Torlek in a WSL2 distribution or on a Linux or macOS host",
        ),
    }
}

pub(crate) fn tmux(tmux: &Result<String, String>) -> Requirement {
    match tmux {
        Ok(version) => Requirement::available("tmux", format!("tmux {version}")),
        Err(reason) => Requirement::needs(
            "tmux",
            Status::MissingPrerequisite,
            format!("{reason}; sessions are refused"),
            format!("install the tmux version named here on the daemon's PATH, {RESTART}"),
        ),
    }
}

/// No backend enforces a sandbox in this release (#828): `off` runs agents unsandboxed,
/// and `required` refuses every launch, which blocks.
pub(crate) fn sandbox(setting: AgentSandbox) -> Requirement {
    match setting {
        AgentSandbox::Required => Requirement::needs(
            "sandbox",
            Status::MissingPrerequisite,
            format!(
                "no sandbox: {}; agents.sandbox = \"required\" refuses every agent launch",
                cockpit_config::NO_SANDBOX_BACKEND
            ),
            "to run agents unsandboxed, with your rights, set agents.sandbox = \"off\" in the owner config and run `torlek config reload`",
        ),
        AgentSandbox::Off => Requirement::available(
            "sandbox",
            format!(
                "no sandbox: agents run UNSANDBOXED (agents.sandbox = \"off\"); {}",
                cockpit_config::UNSANDBOXED_RISK
            ),
        )
        .with_advice(format!(
            "{}; until it is, agents.sandbox = \"required\" refuses every launch",
            cockpit_config::NO_SANDBOX_BACKEND
        )),
    }
}

/// One requirement per provider the daemon wires and one for each usable provider's sign-in.
/// A provider that is not usable is listed with how to install or fix it but does not block,
/// as an unsandboxed owner is: one usable provider is enough (#411). `providers` blocks when none
/// is; a project's preset blocks on the capabilities its steps need (`project::preset`).
pub(crate) fn providers(providers: &[ProviderFacts]) -> Vec<Requirement> {
    let mut requirements: Vec<Requirement> = providers.iter().flat_map(provider).collect();
    if !providers.iter().any(|provider| provider.found.is_ok()) {
        let detail = if providers.is_empty() {
            "the daemon was started without an agent provider"
        } else {
            "no agent provider is usable, so no session can start"
        };
        let is_any_installed = providers
            .iter()
            .any(|provider| matches!(provider.found, Err(ProviderUnusable::Refused { .. })));
        let next = if is_any_installed {
            format!(
                "fix a provider named above, or install another supported agent ({}) on the \
                 daemon's PATH, {RESTART}",
                KNOWN_PROVIDERS.join(", ")
            )
        } else {
            format!(
                "install a supported agent ({}) on the daemon's PATH, {RESTART}",
                KNOWN_PROVIDERS.join(", ")
            )
        };
        requirements.push(Requirement::needs(
            "providers",
            Status::MissingPrerequisite,
            detail,
            next,
        ));
    }
    requirements
}

fn provider(provider: &ProviderFacts) -> Vec<Requirement> {
    let name = format!("provider {}", provider.name);
    let found = match &provider.found {
        Ok(found) => found,
        Err(ProviderUnusable::NotOnPath) => {
            return vec![
                Requirement::available(
                    name,
                    format!("not usable, optional: no `{}` on PATH", provider.name),
                )
                .with_advice(format!(
                    "to use it, install {} on the daemon's PATH, {RESTART}",
                    provider.name
                )),
            ];
        }
        Err(ProviderUnusable::Refused { reason }) => {
            return vec![
                Requirement::available(name, format!("not usable, optional: {reason}"))
                    .with_advice(format!(
                        "to use it, fix what is named here so `{} --version` prints its version \
                         as the daemon's user (report a Torlek bug if it already does), {RESTART}",
                        provider.name
                    )),
            ];
        }
    };
    let version = found.version.as_deref().unwrap_or("of an unknown version");
    let provenance = if found.tested { "tested" } else { "unverified" };
    let detail = format!(
        "{version} ({provenance}); reports {}",
        listed(&found.capabilities)
    );
    vec![
        Requirement::available(name, detail),
        match found.sign_in.signed_in {
            Some(true) => Requirement::available(
                format!("provider {} sign-in", provider.name),
                &found.sign_in.detail,
            ),
            signed_in => Requirement::needs(
                format!("provider {} sign-in", provider.name),
                if signed_in == Some(false) {
                    Status::NeedsAuthorization
                } else {
                    Status::Unknown
                },
                &found.sign_in.detail,
                &found.sign_in.next,
            ),
        },
    ]
}

/// Admission works on every host; the controls it cannot enforce are requested only, which
/// the detail names (report 10).
pub(crate) fn resource_controls(controls: &[ControlFacts]) -> Requirement {
    if controls.is_empty() {
        return Requirement::needs(
            "resource controls",
            Status::Unknown,
            "the daemon reported no resource controls",
            format!("check the daemon's log for its resource probe, {RESTART}"),
        );
    }
    let enforced: Vec<String> = controls
        .iter()
        .filter_map(|control| {
            let how = control.enforced.as_ref().ok()?;
            Some(format!("{} ({how})", control.control))
        })
        .collect();
    let requested_only: Vec<String> = controls
        .iter()
        .filter_map(|control| {
            let why = control.enforced.as_ref().err()?;
            Some(format!("{} ({why})", control.control))
        })
        .collect();
    let mut detail = format!("enforced: {}", listed(&enforced));
    if !requested_only.is_empty() {
        detail.push_str(&format!(
            "; requested only, not enforced here: {}",
            requested_only.join(", ")
        ));
    }
    Requirement::available("resource controls", detail)
}

pub(crate) fn listed(items: &[String]) -> String {
    if items.is_empty() {
        return "none".to_owned();
    }
    items.join(", ")
}
