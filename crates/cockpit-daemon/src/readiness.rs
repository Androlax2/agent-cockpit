//! Serves `readiness` (#50): gathers each fact from its owner (the tmux and platform the
//! start found, the providers it wired, resource admission, the workflow presets, the
//! project registry and the configuration), lets `cockpit-readiness` assess them, and converts
//! the report to the wire. The statuses and next actions live in that crate; this file maps.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cockpit_protocol::{ErrorCode, Outcome, ReadinessStatusView, Reply, RequirementView};
use cockpit_readiness::{
    CheckoutFacts, ControlFacts, HostFacts, PresetFacts, ProjectFacts, ProjectNeeds, ProviderFacts,
    ProviderFound, Report, SignInFacts, Status, Trust,
};
use cockpit_resource_admission::{Availability, Backend};

use crate::routes::{Route, WorkClass, refusal};

/// What the readiness report is gathered from.
#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) database: Arc<cockpit_turso::Database>,
    pub(crate) sessions: Arc<cockpit_sessions::Sessions>,
    pub(crate) admission: Arc<Backend>,
    /// The sandbox launches go through, with the platform the start detected.
    pub(crate) sandbox: cockpit_sandbox::Sandbox,
    /// The tmux version the start found, or why it refused it.
    pub(crate) tmux: Result<String, String>,
    /// Every provider the start tried, wired or not.
    pub(crate) providers: Vec<ProviderFacts>,
    pub(crate) sign_in_providers: Vec<Arc<dyn cockpit_provider_api::Provider>>,
    /// Read on each request, so a changed configuration shows at once.
    pub(crate) environment: cockpit_config::Environment,
    /// Copy out of the viewer and CMD+V's image paste into it (#848).
    pub(crate) manager: Arc<crate::manager::Manager>,
}

/// The facts a wired provider reports about itself.
pub(crate) fn provider_found(provider: &dyn cockpit_provider_api::Provider) -> ProviderFacts {
    let capabilities = provider.capabilities();
    let reported: Vec<_> = cockpit_provider_api::Capability::ALL
        .into_iter()
        .filter_map(|capability| Some((capability, capabilities.provenance(capability)?)))
        .collect();
    let tested = !reported.is_empty()
        && reported.iter().all(|(_, provenance)| {
            matches!(provenance, cockpit_provider_api::Provenance::Tested { .. })
        });
    ProviderFacts {
        name: provider.name().as_str().to_owned(),
        found: Ok(ProviderFound {
            version: provider.version().map(str::to_owned),
            tested,
            capabilities: reported
                .into_iter()
                .map(|(capability, _)| capability.as_str().to_owned())
                .collect(),
            // Startup diagnostics do not probe authentication. Readiness refreshes it
            // on its bulk worker for every request, so a login needs no daemon restart.
            sign_in: sign_in_fact(cockpit_provider_api::SignIn::unknown(
                "sign-in has not been probed",
                "run `torlek doctor`",
            )),
        }),
    }
}

/// Sign-in and Docker are probed in the request, so the work runs in the bulk lane.
pub(crate) fn route(project: Option<String>, context: Context) -> Route {
    let project = match project.map(PathBuf::from) {
        Some(dir) if !dir.is_absolute() => {
            return Route::Answer(refusal(
                ErrorCode::InvalidArgument,
                &format!("{} is not an absolute directory", dir.display()),
            ));
        }
        other => other,
    };
    Route::Run {
        class: WorkClass::Bulk,
        job: Box::pin(async move {
            let project = match project {
                Some(dir) => match tokio::task::spawn_blocking(move || {
                    dir.canonicalize()
                        .map_err(|error| format!("{}: {error}", dir.display()))
                })
                .await
                {
                    Ok(Ok(dir)) => Some(dir),
                    Ok(Err(problem)) => return refusal(ErrorCode::InvalidArgument, &problem),
                    Err(error) => return refusal(ErrorCode::Internal, &error.to_string()),
                },
                None => None,
            };
            match requirements(project, context).await {
                Ok(requirements) => Outcome::Ok(Reply::Readiness { requirements }),
                Err(error) => {
                    tracing::error!(%error, "the readiness report failed");
                    refusal(
                        ErrorCode::Internal,
                        &format!("the readiness report failed: {error}"),
                    )
                }
            }
        }),
    }
}

/// The readiness report as `torlek doctor` shows it; also the support bundle's section.
pub(crate) async fn requirements(
    project: Option<PathBuf>,
    context: Context,
) -> Result<Vec<RequirementView>, String> {
    let report = report(project, context).await?;
    Ok(report
        .requirements
        .into_iter()
        .map(requirement_view)
        .collect())
}

async fn report(project: Option<PathBuf>, context: Context) -> Result<Report, String> {
    let clipboard = {
        let context = context.clone();
        tokio::task::spawn_blocking(move || clipboard_requirements(&context))
            .await
            .map_err(|error| error.to_string())?
    };
    let mut host = host_facts(&context);
    let providers = context.sign_in_providers.clone();
    host.providers = tokio::task::spawn_blocking(move || {
        let mut facts = host.providers;
        for provider in providers {
            if let Some(found) = facts
                .iter_mut()
                .find(|fact| fact.name == provider.name().as_str())
                .and_then(|fact| fact.found.as_mut().ok())
            {
                found.sign_in = sign_in_fact(provider.sign_in());
            }
        }
        facts
    })
    .await
    .map_err(|error| error.to_string())?;
    let (dir, checkout) = match &project {
        Some(dir) => {
            let (dir, checkout) = checkout_of(dir, &context.database).await?;
            (Some(dir), checkout)
        }
        None => (None, None),
    };
    let environment = context.environment.clone();
    let config_dir = dir.clone();
    let config = tokio::task::spawn_blocking(move || {
        cockpit_config::load(&environment, config_dir.as_deref(), &[])
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(dir) = dir else {
        return Ok(with_daemon_lines(
            cockpit_readiness::assess(&host, &config, None),
            &context,
            clipboard,
        ));
    };
    let checkout = match (&config, checkout) {
        (Ok(config), Some(checkout)) => Some(checkout_facts(config, checkout)),
        _ => None,
    };
    let docker = match &config {
        Ok(config) if cockpit_readiness::needs_docker(config) => Some(
            tokio::task::spawn_blocking(probe_docker)
                .await
                .map_err(|error| error.to_string())?,
        ),
        _ => None,
    };
    let capacity = context.admission.capacity();
    let project = ProjectFacts {
        dir,
        checkout,
        presets: presets(),
        capacity: ProjectNeeds {
            cpu_millis: capacity.cpu_millis,
            memory_mib: capacity.memory_mib,
        },
        docker,
    };
    Ok(with_daemon_lines(
        cockpit_readiness::assess(&host, &config, Some(&project)),
        &context,
        clipboard,
    ))
}

/// `report` with what only the daemon knows: the disk guard's reading and `clipboard`.
fn with_daemon_lines(
    mut report: Report,
    context: &Context,
    clipboard: [cockpit_readiness::Requirement; 2],
) -> Report {
    report
        .requirements
        .extend(disk_requirement(&context.admission));
    report.requirements.extend(clipboard);
    report
}

/// Copy out of a session and image paste into it (#848): optional, so neither blocks. Runs
/// tmux.
fn clipboard_requirements(context: &Context) -> [cockpit_readiness::Requirement; 2] {
    let restart = "then restart the daemon";
    let copies_out = match context.manager.passes_copies_out() {
        Ok(true) => cockpit_readiness::Requirement::available(
            "clipboard out",
            "a copy in the session in the viewer (copy mode: the prefix twice, then [) reaches \
             your terminal with OSC 52, when it accepts that, as kitty does",
        ),
        Ok(false) => cockpit_readiness::Requirement::available(
            "clipboard out",
            "not usable, optional: this tmux has no get-clipboard option, so copies stop at \
             the viewer rather than let panes read the clipboard",
        )
        .with_advice(format!(
            "to use it, install a tmux with get-clipboard (3.8 has it), {restart}"
        )),
        Err(reason) => cockpit_readiness::Requirement::available(
            "clipboard out",
            format!("not usable, optional: {reason}"),
        )
        .with_advice("to use it, make tmux available as its own line says"),
    };
    [copies_out, image_paste_requirement(context)]
}

fn image_paste_requirement(context: &Context) -> cockpit_readiness::Requirement {
    let name = "clipboard in";
    let reader = context.manager.clipboard_reader();
    let Some(found) = crate::clipboard::find_program(reader) else {
        return cockpit_readiness::Requirement::available(
            name,
            format!(
                "not usable, optional: no `{}` on the daemon's PATH",
                reader.display()
            ),
        )
        .with_advice(
            "to use it, install wl-clipboard on the daemon's PATH, then restart the daemon",
        );
    };
    let desktop = context.sessions.desktop_environment();
    let Some(display) = crate::clipboard::wayland_display(&desktop) else {
        return cockpit_readiness::Requirement::available(
            name,
            "not usable, optional: the daemon knows no Wayland display to read the clipboard on",
        )
        .with_advice(
            "to use it, run `torlek desktop-session import` in a terminal of your desktop",
        );
    };
    cockpit_readiness::Requirement::available(
        name,
        format!(
            "CMD+V in the viewer pastes the clipboard's PNG into the session on screen ({} on \
             Wayland display {})",
            found.display(),
            display.to_string_lossy()
        ),
    )
}

/// The low-disk guard's last reading (#406); nothing before its first reading or when the
/// owner turned it off.
fn disk_requirement(backend: &Backend) -> Option<cockpit_readiness::Requirement> {
    let space = backend.disk_space()?;
    let detail = format!("{} MiB free, floor {} MiB", space.free_mib, space.floor_mib);
    if !space.is_low() {
        return Some(cockpit_readiness::Requirement::available("disk", detail));
    }
    Some(cockpit_readiness::Requirement::needs(
        "disk",
        cockpit_readiness::Status::MissingPrerequisite,
        format!("{detail}: new launches and heavy jobs wait"),
        "free space, or review what `torlek storage` could prune; waiting work resumes by itself",
    ))
}

fn sign_in_fact(fact: cockpit_provider_api::SignIn) -> SignInFacts {
    use cockpit_provider_api::{Provenance, SignInStatus};
    let provenance = match fact.provenance {
        Provenance::Tested { version } => format!("tested on {version}"),
        Provenance::Discovered => "unverified".to_owned(),
    };
    let (signed_in, detail, next) = match fact.status {
        SignInStatus::SignedIn => (Some(true), "signed in".to_owned(), String::new()),
        SignInStatus::NotSignedIn { next } => (Some(false), "not signed in".to_owned(), next),
        SignInStatus::Unknown { reason, next } => (None, reason, next),
    };
    SignInFacts {
        signed_in,
        detail: format!("{detail} ({provenance})"),
        next,
    }
}

fn host_facts(context: &Context) -> HostFacts {
    HostFacts {
        platform: platform(context.sandbox.platform()),
        tmux: context.tmux.clone(),
        sandbox_setting: context.sessions.owner_settings().sandbox,
        sandbox_backend: match context.sandbox.backend() {
            cockpit_sandbox::Backend::Native(native) => Ok(native.describe()),
            cockpit_sandbox::Backend::Missing { reason } => Err(reason.clone()),
        },
        providers: context.providers.clone(),
        controls: context
            .admission
            .support()
            .into_iter()
            .map(|support| ControlFacts {
                control: support.control.as_str().to_owned(),
                enforced: match support.availability {
                    Availability::Supported { how } => Ok(how),
                    Availability::Unsupported { why } => Err(why),
                },
            })
            .collect(),
    }
}

fn platform(platform: &cockpit_sandbox::Platform) -> cockpit_readiness::Platform {
    match platform {
        cockpit_sandbox::Platform::Linux => cockpit_readiness::Platform::Linux,
        cockpit_sandbox::Platform::MacOs => cockpit_readiness::Platform::MacOs,
        cockpit_sandbox::Platform::Wsl2 { windows_drives, .. } => {
            cockpit_readiness::Platform::Wsl2 {
                windows_drives: windows_drives.clone(),
            }
        }
        cockpit_sandbox::Platform::Unsupported(name) => {
            cockpit_readiness::Platform::Unsupported(name.clone())
        }
    }
}

/// The registered checkout holding `dir` (canonical), the innermost when clones nest, and the directory
/// the project's configuration is read from: that checkout's root, else `dir` itself.
async fn checkout_of(
    dir: &Path,
    database: &cockpit_turso::Database,
) -> Result<(PathBuf, Option<cockpit_projects::Checkout>), String> {
    let read = database.read().await.map_err(|error| error.to_string())?;
    let checkouts = cockpit_projects::checkouts(&read, None)
        .await
        .map_err(|error| error.to_string())?;
    let checkout = checkouts
        .into_iter()
        .filter(|checkout| dir.starts_with(&checkout.path))
        .max_by_key(|checkout| checkout.path.components().count());
    match checkout {
        Some(checkout) => Ok((checkout.path.clone(), Some(checkout))),
        None => Ok((dir.to_path_buf(), None)),
    }
}

/// Trust as `cockpit-projects` decides it, from the owner's `projects.trusted` read now.
fn checkout_facts(
    config: &cockpit_config::EffectiveConfig,
    checkout: cockpit_projects::Checkout,
) -> CheckoutFacts {
    let trusted = cockpit_projects::resolve_trusted_roots(config.trusted_projects.value.clone());
    let trust = match cockpit_projects::trust(&checkout, &trusted) {
        cockpit_projects::Trust::Trusted => Trust::Trusted,
        cockpit_projects::Trust::Inherited { from } => Trust::Inherited { from },
        cockpit_projects::Trust::Untrusted => Trust::Untrusted,
        cockpit_projects::Trust::NeverForReview => Trust::NeverForReview,
    };
    CheckoutFacts {
        project: checkout.project.as_str().to_owned(),
        project_root: checkout.project_root,
        trust,
    }
}

/// Every preset with the capabilities its agent steps need, by their protocol names.
fn presets() -> Vec<PresetFacts> {
    cockpit_workflows::presets()
        .into_iter()
        .map(|preset| {
            let mut capabilities: Vec<String> = Vec::new();
            for capability in preset
                .steps
                .iter()
                .flat_map(|step| step.required_capabilities)
            {
                let name = capability.as_str().to_owned();
                if !capabilities.contains(&name) {
                    capabilities.push(name);
                }
            }
            PresetFacts {
                name: preset.name.to_owned(),
                capabilities,
            }
        })
        .collect()
}

/// The `docker` on the daemon's `PATH`, asked for its engine.
fn probe_docker() -> Result<cockpit_readiness::DockerEngine, String> {
    cockpit_readiness::probe_docker(Path::new("docker"), cockpit_readiness::DOCKER_DEADLINE)
}

fn requirement_view(requirement: cockpit_readiness::Requirement) -> RequirementView {
    RequirementView {
        name: requirement.name,
        status: match requirement.status {
            Status::Available => ReadinessStatusView::Available,
            Status::NeedsAuthorization => ReadinessStatusView::NeedsAuthorization,
            Status::MissingPrerequisite => ReadinessStatusView::MissingPrerequisite,
            Status::Unsupported => ReadinessStatusView::Unsupported,
            Status::Unknown => ReadinessStatusView::Unknown,
        },
        detail: requirement.detail,
        next: requirement.next,
    }
}
