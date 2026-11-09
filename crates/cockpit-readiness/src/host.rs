//! The requirements every host has, project or not.

use cockpit_config::{
    AccountCredentialState, AccountLoginState, AgentSandbox, ConfigFile, DEFAULT_ACCOUNT,
    EffectiveConfig, KNOWN_PROVIDERS, ProviderCredentialState, Sourced,
};

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

/// How the owner's setting treats launches with the `backend` the daemon probed: `off` runs
/// agents unsandboxed; `required` runs them in the native sandbox, or refuses every launch
/// when the host has none, which blocks. It blocks too while the owner's shared desktop
/// session is one the sandbox refuses (`shared_desktop_refusal`), as every launch is (#965).
pub(crate) fn sandbox(
    setting: AgentSandbox,
    backend: &Result<String, String>,
    shared_desktop_refusal: Option<&str>,
) -> Requirement {
    match (setting, backend) {
        (AgentSandbox::Required, Ok(_)) if let Some(refusal) = shared_desktop_refusal => {
            Requirement::needs(
                "sandbox",
                Status::MissingPrerequisite,
                format!("{refusal}; agents.sandbox = \"required\" refuses every agent launch"),
                "set agents.desktop_session = \"withheld\" in the owner config and run `torlek config reload`; to run agents unsandboxed, with your rights, set agents.sandbox = \"off\" instead",
            )
        }
        (AgentSandbox::Required, Ok(backend)) => Requirement::available(
            "sandbox",
            format!(
                "sandboxed: every agent runs in {backend} and reaches the network only through \
                 its own egress proxy; a host it may not reach is refused at once and you decide \
                 with `torlek network`"
            ),
        ),
        (AgentSandbox::Required, Err(reason)) => Requirement::needs(
            "sandbox",
            Status::MissingPrerequisite,
            format!(
                "no sandbox: {reason}; agents.sandbox = \"required\" refuses every agent launch"
            ),
            "fix what the detail names and restart the daemon; to run agents unsandboxed, with your rights, set agents.sandbox = \"off\" in the owner config and run `torlek config reload`",
        ),
        (AgentSandbox::Off, backend) => Requirement::available(
            "sandbox",
            format!(
                "no sandbox: agents run UNSANDBOXED (agents.sandbox = \"off\"); {}",
                cockpit_config::UNSANDBOXED_RISK
            ),
        )
        .with_advice(match backend {
            Ok(backend) => format!(
                "agents.sandbox = \"required\" runs them in {backend}; there each session reaches \
                 the network only through its own egress proxy, and you allow once, allow always or \
                 deny a refused host with `torlek network`"
            ),
            Err(reason) => format!(
                "this host has no sandbox ({reason}); until it has, agents.sandbox = \
                 \"required\" refuses every launch"
            ),
        }),
    }
}

/// The owner's configuration now says `written`, and the daemon still applies `applied`, which
/// it read before (#963): what the owner chose is not in force, so this blocks, and the next
/// action is the command that applies the file rather than advice to write what it already
/// says.
pub(crate) fn unapplied_sandbox(
    written: &Sourced<AgentSandbox>,
    applied: AgentSandbox,
    backend: &Result<String, String>,
) -> Requirement {
    let still = match (applied, backend) {
        (AgentSandbox::Off, _) => format!(
            "agents still run UNSANDBOXED; {}",
            cockpit_config::UNSANDBOXED_RISK
        ),
        (AgentSandbox::Required, Ok(backend)) => format!("every agent still runs in {backend}"),
        (AgentSandbox::Required, Err(reason)) => {
            format!("every agent launch is still refused: {reason}")
        }
    };
    let refused_once_applied = match (written.value, backend) {
        (AgentSandbox::Required, Err(reason)) => format!(
            "; this host has no sandbox ({reason}), so \"required\" then refuses every launch \
             until that is fixed"
        ),
        _ => String::new(),
    };
    Requirement::needs(
        "sandbox",
        Status::MissingPrerequisite,
        format!(
            "agents.sandbox = \"{}\" ({}) is not in force: the running daemon still applies \
             \"{}\", which it read before the setting changed, so {still}",
            written.value.as_str(),
            written.source,
            applied.as_str()
        ),
        format!(
            "run `torlek config reload`, which applies the owner config to the agents launched \
             after it; restarting the daemon applies it too{refused_once_applied}"
        ),
    )
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
        let is_any_installed = providers.iter().any(|provider| {
            matches!(
                provider.found,
                Err(ProviderUnusable::Refused { .. } | ProviderUnusable::DeclarationRefused { .. })
            )
        });
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
        Err(ProviderUnusable::DeclarationRefused { reason }) => {
            return vec![
                Requirement::available(
                    name,
                    format!("not usable, optional: its sandbox declaration is refused: {reason}"),
                )
                .with_advice("the declaration is Torlek's adapter's own: report a Torlek bug"),
            ];
        }
    };
    let version = found.version.as_deref().unwrap_or("of an unknown version");
    let provenance = match (&found.probe, found.tested) {
        (_, true) => "tested".to_owned(),
        (Some(probe), false) if probe.is_clean => "probed, not tested".to_owned(),
        (Some(probe), false) => format!("probe failed: {}", probe.summary),
        (None, false) => "unverified".to_owned(),
    };
    let detail = format!(
        "{version} ({provenance}); reports {}",
        listed(&found.capabilities)
    );
    let probe_failed = found.probe.as_ref().is_some_and(|probe| !probe.is_clean);
    let mut requirements = vec![
        if probe_failed {
            Requirement::available(name, detail).with_advice(format!(
                "this {0} release lacks something Torlek relies on: update Torlek, or use a {0} \
                 version Torlek lists as tested (`torlek doctor` shows it)",
                provider.name
            ))
        } else {
            Requirement::available(name, detail)
        },
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
    ];
    let launches_on = launch_account(&found.sandbox_credentials);
    requirements.extend(
        found
            .sandbox_credentials
            .iter()
            .map(|account| sandbox_credential(&provider.name, account, launches_on)),
    );
    requirements
}

/// The account a sandboxed launch runs on in a project holding no login grant, as the daemon
/// picks it (#59): the first that holds a credential, passing over one holding nothing or a
/// signed-in login, unless what it holds is unusable and refuses the launch. The rows judge
/// blocking by this project, the strictest; a login's own row names the projects granted it.
fn launch_account(accounts: &[AccountCredentialState]) -> Option<&str> {
    for account in accounts {
        match (&account.state, &account.login) {
            (
                ProviderCredentialState::Missing { .. },
                AccountLoginState::NotKept
                | AccountLoginState::NotSignedIn
                | AccountLoginState::SignedIn { .. },
            ) => {}
            (
                ProviderCredentialState::Configured { .. },
                AccountLoginState::NotKept | AccountLoginState::NotSignedIn,
            ) => return Some(account.account.as_str()),
            _ => return None,
        }
    }
    None
}

/// The sign-in a sandboxed session of `provider` gets: the owner's credential the session
/// proxy adds on its hosts, the login account's sign-in bound in its home, or what to write
/// when there is neither (#858, #968, #59). Without one, the harness asks for a sign-in at
/// every new session and cannot store it. The `default` account keeps the row #968 named;
/// another account is named in its row (#59). An account listed after the one launches run
/// on, `launches_on`, blocks nothing, which its row says.
fn sandbox_credential(
    provider: &str,
    account: &AccountCredentialState,
    launches_on: Option<&str>,
) -> Requirement {
    let name = if account.account == DEFAULT_ACCOUNT {
        format!("provider {provider} sandboxed sign-in")
    } else {
        format!(
            "provider {provider} account {} sandboxed sign-in",
            account.account
        )
    };
    let refused = |detail: String, next: String| match launches_on {
        Some(used) => Requirement::needs(
            name.clone(),
            Status::NeedsAuthorization,
            format!("{detail}, so sandboxed launches run on account {used} instead"),
            next,
        ),
        None => Requirement::needs(
            name.clone(),
            Status::MissingPrerequisite,
            format!("{detail}, so its sandboxed sessions do not launch"),
            next,
        ),
    };
    match (&account.state, &account.login) {
        (ProviderCredentialState::Unusable { reason }, _) => refused(
            format!("unusable ({reason})"),
            "fix the file it names, or remove it".to_owned(),
        ),
        (_, AccountLoginState::Unusable { reason }) => refused(
            reason.clone(),
            format!(
                "run `torlek account sign-in {provider} {}` again",
                account.account
            ),
        ),
        (ProviderCredentialState::Configured { .. }, AccountLoginState::SignedIn { .. }) => {
            refused(
                "holds both a credential file and a signed-in login".to_owned(),
                "remove the one it should not use".to_owned(),
            )
        }
        (ProviderCredentialState::Configured { variable, path }, _) => Requirement::available(
            name,
            format!(
                "{} ({variable}): the session proxy adds it on {provider}'s hosts; the agent \
                 holds a placeholder",
                path.display()
            ),
        ),
        (
            ProviderCredentialState::Missing { .. },
            AccountLoginState::SignedIn { file, granted_to },
        ) => {
            if granted_to.is_empty() {
                Requirement::needs(
                    name,
                    Status::NeedsAuthorization,
                    format!(
                        "signed-in login {}, but no project holds a grant for it, so no session \
                         uses it",
                        file.display()
                    ),
                    format!(
                        "let a project use it with `torlek account grant PROJECT {provider} {}`",
                        account.account
                    ),
                )
            } else {
                Requirement::available(
                    name,
                    format!(
                        "signed-in login {}: sessions of {} {} use it, one at a time",
                        file.display(),
                        if granted_to.len() == 1 {
                            "project"
                        } else {
                            "projects"
                        },
                        granted_to.join(", ")
                    ),
                )
            }
        }
        (ProviderCredentialState::Missing { files }, login) => {
            let listed_files = listed(
                &files
                    .iter()
                    .map(|file| file.display().to_string())
                    .collect::<Vec<_>>(),
            );
            // The credential verbs save the default account's (#1251).
            let write_file = if account.account == DEFAULT_ACCOUNT {
                format!(
                    "save one with `torlek provider credential set {provider} --variable NAME`, \
                     NAME being {}; it writes one of {listed_files} with mode 0600",
                    variable_names(files).join(" or ")
                )
            } else {
                format!(
                    "write one credential, on one line, to one of {listed_files}, then `chmod \
                     600` it; the file's name is the variable it stands for"
                )
            };
            let next = if *login == AccountLoginState::NotSignedIn {
                format!(
                    "{write_file}; or sign it in as a login account with `torlek account \
                     sign-in {provider} {account}`, then grant it with `torlek account grant \
                     PROJECT {provider} {account}`",
                    account = account.account
                )
            } else {
                write_file
            };
            Requirement::needs(
                name,
                Status::NeedsAuthorization,
                match launches_on {
                    Some(used) => {
                        format!("none: sandboxed launches run on account {used} instead")
                    }
                    None => format!(
                        "none: a sandboxed {provider} session never reads your own sign-in, so \
                         it asks for one at every new session"
                    ),
                },
                next,
            )
        }
    }
}

/// The variables `files` stand for: each file is named after its variable.
fn variable_names(files: &[std::path::PathBuf]) -> Vec<String> {
    files
        .iter()
        .filter_map(|file| file.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect()
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
