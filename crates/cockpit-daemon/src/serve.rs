//! What the `cockpit-daemon` binary runs once its arguments are read: the owner's
//! configuration, the provider adapters, the wiring and the daemon until it stops. The
//! end-to-end tests' daemon (`examples/cockpit-e2e-daemon.rs`, #300) runs the same with
//! shorter clocks, so the product's configuration floors never move for a test.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cockpit_provider_api::{Provenance, Provider};

use crate::{Instance, Options, Routes, StartError, Wiring, session_settings};

/// The folder under the state directory the OpenCode plugin is written to at every start;
/// agents read it, so it holds nothing else.
const OPENCODE_PLUGIN_DIR: &str = "opencode-plugin";

/// How often the product's sessions pass (exits, idle sleep, expiry) runs.
pub const SESSION_TICK: Duration = Duration::from_secs(30);

/// Variables of the daemon's own environment every agent starts with, where set, beside its
/// XDG layout; the provider adds its own after them. The desktop session's
/// ([`cockpit_protocol::DESKTOP_SESSION_VARIABLES`]) follow while the owner shares them.
const AGENT_BASE_VARIABLES: &[&str] = &[
    "HOME", "USER", "LOGNAME", "SHELL", "PATH", "LANG", "LC_ALL", "LC_CTYPE",
];

/// How [`serve`] runs.
pub struct Serve {
    /// The profile whose files, sockets, lock and tmux server this daemon owns (#196).
    pub profile: cockpit_config::Sourced<cockpit_config::Profile>,
    /// Also print the log to stderr.
    pub foreground: bool,
    /// The installation's directory: `cockpit` and the adapters' bridge programs
    /// (`cockpit-claude-hook`, `cockpit-codex-hook`, `cockpit-codex-session`,
    /// `cockpit-opencode-bridge`) beside the daemon.
    pub programs_dir: PathBuf,
    /// [`SESSION_TICK`] in the product.
    pub session_tick: Duration,
    /// Replaces the configured idle window. `None` in the product; only the end-to-end
    /// tests' daemon sets it, below the configuration's floor.
    pub idle_sleep_after: Option<Duration>,
}

/// This program's own path, the start of where its installation's programs are.
pub fn own_program() -> Result<PathBuf, StartError> {
    std::env::current_exe().map_err(|source| StartError::Io {
        action: "find the daemon's own program",
        path: "/proc/self/exe".into(),
        source,
    })
}

/// The profile this daemon program serves: `--profile` (`flag`), else `$COCKPIT_PROFILE`, else
/// its build's own (`cockpit` for a release, `dev` otherwise).
pub fn own_profile(
    flag: Option<&str>,
) -> Result<cockpit_config::Sourced<cockpit_config::Profile>, StartError> {
    let variable = std::env::var_os(cockpit_config::PROFILE_VARIABLE);
    let choice = cockpit_config::ProfileChoice {
        flag,
        variable: variable.as_deref(),
        command_name: None,
    };
    cockpit_config::resolve_profile(choice, cockpit_config::Build::of_program(&own_program()?))
        .map_err(StartError::Config)
}

/// Runs the daemon in this process until it stops; prints `ready <socket>` once it accepts.
pub fn serve(serve: Serve) -> Result<(), StartError> {
    let profile = serve.profile.value.clone();
    let environment = cockpit_config::Environment::from_process(serve.profile);
    let options = Options::from_environment(&environment)?;
    // The lock comes first, so a second daemon reports that one is running instead of
    // failing on the log or database lock behind it.
    let instance = Instance::acquire(options)?;
    let mut log_options = cockpit_diagnostics::Options::new(
        cockpit_diagnostics::log_dir_in(&instance.options().state_dir, "daemon")?,
        cockpit_diagnostics::Directives::from_env()?,
    );
    log_options.foreground = serve.foreground;
    let diagnostics = cockpit_diagnostics::start(log_options)?;
    tracing::dispatcher::set_global_default(diagnostics.dispatch().clone())
        .expect("nothing installed a subscriber before startup");
    tracing::info!(
        profile = %profile,
        source = %environment.profile.source,
        "the daemon serves its profile"
    );
    let run_settings =
        crate::run_settings(std::env::vars_os().collect()).map_err(|source| StartError::Io {
            action: "find the daemon's own program",
            path: "/proc/self/exe".into(),
            source,
        })?;
    // Read once: a start file declared or a session setting changed later counts from the
    // next daemon start. The log runs already, so a detached daemon records the refusal.
    let config = cockpit_config::load(&environment, None, &[]).map_err(|error| {
        tracing::error!(%error, "the owner configuration is invalid");
        StartError::Config(error)
    })?;
    let start_files = config.declared_start_files();
    // Discovery runs programs and may block for seconds, so it comes before the runtime.
    let plugin_dir = instance.options().state_dir.join(OPENCODE_PLUGIN_DIR);
    let mut providers: Vec<Arc<dyn Provider>> = Vec::new();
    let mut unavailable_providers = Vec::new();
    match claude_code(&serve.programs_dir) {
        Ok(claude) => providers.push(Arc::new(claude)),
        Err(reason) => unavailable_providers.push(("claude".to_owned(), reason)),
    }
    match codex(&serve.programs_dir) {
        Ok(codex) => providers.push(Arc::new(codex)),
        Err(reason) => unavailable_providers.push(("codex".to_owned(), reason)),
    }
    match opencode(&serve.programs_dir, &plugin_dir) {
        Ok(opencode) => providers.push(Arc::new(opencode)),
        Err(reason) => unavailable_providers.push(("opencode".to_owned(), reason)),
    }
    let sandbox = sandbox(
        instance.options(),
        &environment,
        &[
            serve.programs_dir.clone(),
            plugin_dir,
            // `$COCKPIT_INSTRUCTIONS`, which the launch brief points agents at (#419).
            instance
                .options()
                .state_dir
                .join(crate::agent_context::BUNDLE_DIR),
        ],
        &config,
    );
    let mut sessions = cockpit_sessions::Settings {
        base_environment: own_variables(AGENT_BASE_VARIABLES)
            .chain(
                environment
                    .layout_variables()
                    .into_iter()
                    .map(|(name, value)| (name.into(), value)),
            )
            .collect(),
        start_files,
        sandbox,
        ..session_settings(&config)
    };
    if let Some(window) = serve.idle_sleep_after {
        tracing::warn!(?window, "the idle window is overridden for a test");
        sessions.owner.idle_sleep_after = window;
    }
    let runtime = tokio::runtime::Runtime::new().map_err(StartError::Runtime)?;
    runtime.block_on(async {
        let wiring = Wiring {
            tmux: cockpit_tmux::ServerSpec {
                tmux: "tmux".into(),
                // Each profile its own server, so a `dev` daemon never sees the installed
                // one's sessions (#196).
                socket: cockpit_tmux::Socket::Named(profile.qualified_name()),
                config_path: instance.options().state_dir.join("tmux.conf"),
            },
            git: cockpit_projects::Git::from_path(),
            providers,
            unavailable_providers,
            sessions,
            // The start's values until the owner imports fresher ones (#307).
            desktop_environment: own_variables(&cockpit_protocol::DESKTOP_SESSION_VARIABLES)
                .collect(),
            session_tick: serve.session_tick,
            run_settings,
            environment: environment.clone(),
            scheduling: cockpit_scheduling::Settings::default(),
        };
        let daemon = instance
            .start(wiring, |capabilities| {
                Arc::new(Routes::new(capabilities, environment))
            })
            .await?;
        println!("ready {}", daemon.socket_path().display());
        daemon.wait().await;
        Ok(())
    })
}

/// The sandbox every launch goes through (#61): Greywall and greyproxy as found on the
/// daemon's `PATH` and checked against the pinned builds, with the paths the daemon's layout
/// gives every agent and keeps from it; `bridge_dirs` hold what the adapters' bridges run
/// and load inside it, and the instruction bundle. An engine that is missing or of another build is
/// logged; launches are then refused while the owner requires the sandbox.
fn sandbox(
    options: &Options,
    environment: &cockpit_config::Environment,
    bridge_dirs: &[PathBuf],
    config: &cockpit_config::EffectiveConfig,
) -> cockpit_sandbox::Sandbox {
    let platform = cockpit_sandbox::Platform::detect();
    let settings_dir = options.state_dir.join("sandbox");
    let engine = cockpit_sandbox::GreywallPrograms::on_path(std::env::var_os("PATH").as_ref())
        .and_then(|programs| {
            cockpit_sandbox::Greywall::detect(programs, platform.clone(), settings_dir.clone())
        });
    match (&engine, config.agents_sandbox.value) {
        (Ok(greywall), _) => tracing::info!(
            engine = %greywall.engine(),
            platform = %greywall.platform,
            "the sandbox engine is ready"
        ),
        (Err(reason), cockpit_config::AgentSandbox::Required) => tracing::error!(
            %reason,
            "the sandbox engine is unavailable: every agent launch is refused until it is installed and the daemon restarts"
        ),
        (Err(reason), cockpit_config::AgentSandbox::Off) => tracing::warn!(
            %reason,
            "the sandbox engine is unavailable"
        ),
    }
    if config.agents_sandbox.value == cockpit_config::AgentSandbox::Off {
        tracing::warn!(
            source = %config.agents_sandbox.source,
            "agents run UNSANDBOXED: the owner set agents.sandbox = \"off\""
        );
    }
    let layout_config_dir = environment
        .home_dir()
        .map(|home| {
            cockpit_config::Layout::resolve(environment, &home)
                .config_dir
                .value
        })
        .ok();
    let database = options.database_path();
    let journal = |suffix: &str| {
        let mut path = database.clone().into_os_string();
        path.push(suffix);
        PathBuf::from(path)
    };
    let denied = [
        Some(options.socket_path()),
        Some(journal("-wal")),
        Some(journal("-shm")),
        Some(database.clone()),
        Some(options.state_dir.join("logs")),
        Some(options.executions_dir()),
        Some(settings_dir),
        layout_config_dir,
    ]
    .into_iter()
    .flatten()
    .collect();
    cockpit_sandbox::Sandbox::new(
        platform,
        engine,
        cockpit_sandbox::HostPaths {
            read_only: std::iter::once(
                options
                    .agent_socket_path()
                    .parent()
                    .expect("the agent socket is in its own folder")
                    .to_path_buf(),
            )
            .chain(bridge_dirs.iter().cloned())
            .collect(),
            denied,
        },
    )
}

/// Those of `names` the daemon's own environment sets, never invented (#303).
fn own_variables(names: &[&str]) -> impl Iterator<Item = (OsString, OsString)> {
    names
        .iter()
        .filter_map(|name| Some(((*name).into(), std::env::var_os(name)?)))
}

/// The Claude Code adapter for the `claude` on the daemon's `PATH`, with the hook bridge and
/// the `cockpit` program installed in `programs_dir`. Why not, logged, when there is none or
/// it does not answer its version: the readiness report names it (#50).
fn claude_code(programs_dir: &Path) -> Result<cockpit_claude::ClaudeCode, String> {
    let unavailable = |reason: String| {
        tracing::warn!("provider claude unavailable: {reason}");
        reason
    };
    let Some(program) =
        std::env::var_os("PATH").and_then(|path| cockpit_claude::ClaudeCode::find_on_path(&path))
    else {
        return Err(unavailable("no `claude` on PATH".to_owned()));
    };
    let bridge = cockpit_claude::Bridge {
        hook_program: programs_dir.join("cockpit-claude-hook"),
        cockpit_program: programs_dir.join("cockpit"),
    };
    // Without its bridge the adapter subscribes no hooks and does not report `events`, which
    // `cockpit provider list` shows (#203).
    let bridge = match bridge.check() {
        Ok(()) => Some(bridge),
        Err(reason) => {
            tracing::warn!("provider claude: {reason}, so its activity is not reported");
            None
        }
    };
    let claude = cockpit_claude::ClaudeCode::discover(&program, bridge)
        .map_err(|error| unavailable(error.to_string()))?;
    log_ready(&claude, &claude.provenance());
    Ok(claude)
}

/// The Codex adapter for the `codex` on the daemon's `PATH`, with its session launcher, hook
/// bridge and the `cockpit` program installed in `programs_dir` (#411). Why not, logged, when
/// there is none or it does not answer its version: the readiness report names it (#50).
fn codex(programs_dir: &Path) -> Result<cockpit_codex::Codex, String> {
    let unavailable = |reason: String| {
        tracing::warn!("provider codex unavailable: {reason}");
        reason
    };
    let Some(program) =
        std::env::var_os("PATH").and_then(|path| cockpit_codex::Codex::find_on_path(&path))
    else {
        return Err(unavailable("no `codex` on PATH".to_owned()));
    };
    let bridge = cockpit_codex::Bridge {
        session_program: programs_dir.join("cockpit-codex-session"),
        hook_program: programs_dir.join("cockpit-codex-hook"),
        cockpit_program: programs_dir.join("cockpit"),
    };
    // Without its bridge the adapter launches the TUI alone and reports only `resume`.
    let bridge = match bridge.check() {
        Ok(()) => Some(bridge),
        Err(reason) => {
            tracing::warn!("provider codex: {reason}, so its activity is not reported");
            None
        }
    };
    let codex = cockpit_codex::Codex::discover(&program, bridge)
        .map_err(|error| unavailable(error.to_string()))?;
    log_ready(&codex, &codex.provenance());
    Ok(codex)
}

/// The OpenCode adapter for the `opencode` on the daemon's `PATH`, with its plugin written to
/// `plugin_dir` and the bridge and `cockpit` programs installed in `programs_dir` (#415). Why
/// not, logged, when there is none or it does not answer its version: the readiness report
/// names it (#50).
fn opencode(programs_dir: &Path, plugin_dir: &Path) -> Result<cockpit_opencode::OpenCode, String> {
    let unavailable = |reason: String| {
        tracing::warn!("provider opencode unavailable: {reason}");
        reason
    };
    let Some(program) =
        std::env::var_os("PATH").and_then(|path| cockpit_opencode::OpenCode::find_on_path(&path))
    else {
        return Err(unavailable("no `opencode` on PATH".to_owned()));
    };
    // Without its bridge the adapter loads no plugin and reports neither events nor outcomes,
    // so it offers only `resume`.
    let bridge = write_opencode_plugin(plugin_dir).and_then(|plugin| {
        let bridge = cockpit_opencode::Bridge {
            plugin,
            bridge_program: programs_dir.join("cockpit-opencode-bridge"),
            cockpit_program: programs_dir.join("cockpit"),
        };
        bridge.check().map(|()| bridge)
    });
    let bridge = match bridge {
        Ok(bridge) => Some(bridge),
        Err(reason) => {
            tracing::warn!("provider opencode: {reason}, so its activity is not reported");
            None
        }
    };
    let opencode = cockpit_opencode::OpenCode::discover(&program, bridge)
        .map_err(|error| unavailable(error.to_string()))?;
    log_ready(&opencode, &opencode.provenance());
    Ok(opencode)
}

/// Writes this release's OpenCode plugin into `dir`, replacing the file in one rename so an
/// OpenCode starting meanwhile never loads half of it; its path, or why it could not be.
fn write_opencode_plugin(dir: &Path) -> Result<PathBuf, String> {
    let plugin = dir.join("cockpit-opencode-plugin.js");
    let partial = dir.join("cockpit-opencode-plugin.js.partial");
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&partial, cockpit_opencode::PLUGIN_SOURCE))
        .and_then(|()| std::fs::rename(&partial, &plugin))
        .map_err(|error| format!("cannot write its plugin to {}: {error}", plugin.display()))?;
    Ok(plugin)
}

/// The `provider <name> ready` line the release's install check and the tests read.
fn log_ready(provider: &dyn Provider, provenance: &Provenance) {
    let provenance = match provenance {
        Provenance::Tested { .. } => "tested",
        Provenance::Discovered => "unverified",
    };
    tracing::info!(
        "provider {} ready version={} provenance={provenance}",
        provider.name(),
        provider.version().unwrap_or("unknown")
    );
}
