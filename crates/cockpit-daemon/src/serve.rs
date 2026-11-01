//! What the `cockpit-daemon` binary runs once its arguments are read: the owner's
//! configuration, the Claude Code adapter, the wiring and the daemon until it stops. The
//! end-to-end tests' daemon (`examples/cockpit-e2e-daemon.rs`, #300) runs the same with
//! shorter clocks, so the product's configuration floors never move for a test.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cockpit_provider_api::{Provenance, Provider};

use crate::{Instance, Options, Routes, StartError, Wiring, session_settings};

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
    /// Also print the log to stderr.
    pub foreground: bool,
    /// The installation's directory: `cockpit` and `cockpit-claude-hook` beside the daemon.
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

/// Runs the daemon in this process until it stops; prints `ready <socket>` once it accepts.
pub fn serve(serve: Serve) -> Result<(), StartError> {
    let environment = cockpit_config::Environment::from_process();
    let options = Options::from_environment(&environment)?;
    // The lock comes first, so a second daemon reports that one is running instead of
    // failing on the log or database lock behind it.
    let instance = Instance::acquire(options)?;
    let state_home = instance
        .options()
        .state_dir
        .parent()
        .expect("the state directory is <state home>/cockpit");
    let mut log_options = cockpit_diagnostics::Options::new(
        cockpit_diagnostics::log_dir_in(state_home, "daemon")?,
        cockpit_diagnostics::Directives::from_env()?,
    );
    log_options.foreground = serve.foreground;
    let diagnostics = cockpit_diagnostics::start(log_options)?;
    tracing::dispatcher::set_global_default(diagnostics.dispatch().clone())
        .expect("nothing installed a subscriber before startup");
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
    let providers: Vec<Arc<dyn Provider>> = claude_code(&serve.programs_dir)
        .into_iter()
        .map(|claude| Arc::new(claude) as Arc<dyn Provider>)
        .collect();
    let mut sessions = cockpit_sessions::Settings {
        base_environment: own_variables(AGENT_BASE_VARIABLES)
            .chain(
                environment
                    .layout_variables()
                    .map(|(name, value)| (name.into(), value.clone())),
            )
            .collect(),
        start_files,
        ..session_settings(&config)
    };
    if let Some(window) = serve.idle_sleep_after {
        tracing::warn!(?window, "the idle window is overridden for a test");
        sessions.idle_sleep_after = window;
    }
    let runtime = tokio::runtime::Runtime::new().map_err(StartError::Runtime)?;
    runtime.block_on(async {
        let wiring = Wiring {
            tmux: cockpit_tmux::ServerSpec {
                tmux: "tmux".into(),
                socket: cockpit_tmux::Socket::cockpit(),
                config_path: instance.options().state_dir.join("tmux.conf"),
            },
            git: cockpit_projects::Git::from_path(),
            providers,
            sessions,
            // The start's values until the owner imports fresher ones (#307).
            desktop_environment: own_variables(&cockpit_protocol::DESKTOP_SESSION_VARIABLES)
                .collect(),
            session_tick: serve.session_tick,
            run_settings,
            environment: environment.clone(),
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

/// Those of `names` the daemon's own environment sets, never invented (#303).
fn own_variables(names: &[&str]) -> impl Iterator<Item = (OsString, OsString)> {
    names
        .iter()
        .filter_map(|name| Some(((*name).into(), std::env::var_os(name)?)))
}

/// The Claude Code adapter for the `claude` on the daemon's `PATH`, with the hook bridge and
/// the `cockpit` program installed in `programs_dir`. `None`, logged, when there is none or
/// it does not answer its version.
fn claude_code(programs_dir: &Path) -> Option<cockpit_claude::ClaudeCode> {
    let unavailable = |reason: &dyn std::fmt::Display| {
        tracing::warn!("provider claude unavailable: {reason}");
    };
    let Some(program) =
        std::env::var_os("PATH").and_then(|path| cockpit_claude::ClaudeCode::find_on_path(&path))
    else {
        unavailable(&"no `claude` on PATH");
        return None;
    };
    let bridge = cockpit_claude::Bridge {
        hook_program: programs_dir.join("cockpit-claude-hook"),
        cockpit_program: programs_dir.join("cockpit"),
    };
    let missing: Vec<String> = [&bridge.hook_program, &bridge.cockpit_program]
        .into_iter()
        .filter(|program| !program.is_file())
        .map(|program| program.display().to_string())
        .collect();
    // Without its bridge the adapter subscribes no hooks and does not report `events`.
    let bridge = if missing.is_empty() {
        Some(bridge)
    } else {
        tracing::warn!(
            "provider claude: {} missing, so its activity is not reported",
            missing.join(" and ")
        );
        None
    };
    let claude = match cockpit_claude::ClaudeCode::discover(&program, bridge) {
        Ok(claude) => claude,
        Err(error) => {
            unavailable(&error);
            return None;
        }
    };
    let provenance = match claude.provenance() {
        Provenance::Tested { .. } => "tested",
        Provenance::Discovered => "unverified",
    };
    tracing::info!(
        "provider claude ready version={} provenance={provenance}",
        claude.version().unwrap_or("unknown")
    );
    Some(claude)
}
