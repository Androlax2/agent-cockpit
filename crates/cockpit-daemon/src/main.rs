//! `cockpit-daemon`: this user's one Cockpit daemon (report 03). `--foreground` also prints
//! the log to stderr. It prints `ready <socket>` on stdout once it accepts requests.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;

use std::time::Duration;

use cockpit_daemon::{
    Instance, Options, RUN_LEADER_FLAG, Routes, StartError, Wiring, session_settings,
};
use cockpit_provider_api::{Provenance, Provider};

const USAGE: &str = "usage: cockpit-daemon [--foreground]";
/// Variables of the daemon's own environment every agent starts with, where set, beside its
/// XDG layout; the provider adds its own after them. The display ones let the agent's tools
/// reach the owner's desktop session, notifications first (#303).
const AGENT_BASE_VARIABLES: [&str; 11] = [
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "PATH",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
];

fn main() -> ExitCode {
    let mut raw_arguments = std::env::args_os().skip(1);
    // The daemon starts itself in this mode as the session leader of each approved run.
    if raw_arguments.next().as_deref() == Some(RUN_LEADER_FLAG.as_ref()) {
        return cockpit_processes::run_session_leader(&raw_arguments.collect::<Vec<_>>());
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let foreground = match arguments.as_slice() {
        [] => false,
        [flag] if flag == "--foreground" => true,
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match run(foreground) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cockpit-daemon: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(foreground: bool) -> Result<(), StartError> {
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
    log_options.foreground = foreground;
    let diagnostics = cockpit_diagnostics::start(log_options)?;
    tracing::dispatcher::set_global_default(diagnostics.dispatch().clone())
        .expect("nothing installed a subscriber before startup");
    let run_settings =
        cockpit_daemon::run_settings(std::env::vars_os().collect()).map_err(|source| {
            StartError::Io {
                action: "find the daemon's own program",
                path: "/proc/self/exe".into(),
                source,
            }
        })?;
    // Read once: a start file declared or a session setting changed later counts from the
    // next daemon start. The log runs already, so a detached daemon records the refusal.
    let config = cockpit_config::load(&environment, None, &[]).map_err(|error| {
        tracing::error!(%error, "the owner configuration is invalid");
        StartError::Config(error)
    })?;
    let start_files = config.declared_start_files();
    // Discovery runs programs and may block for seconds, so it comes before the runtime.
    let daemon_program = std::env::current_exe().map_err(|source| StartError::Io {
        action: "find the daemon's own program",
        path: "/proc/self/exe".into(),
        source,
    })?;
    let providers: Vec<Arc<dyn Provider>> = claude_code(&daemon_program)
        .into_iter()
        .map(|claude| Arc::new(claude) as Arc<dyn Provider>)
        .collect();
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
            sessions: cockpit_sessions::Settings {
                base_environment: AGENT_BASE_VARIABLES
                    .iter()
                    .filter_map(|name| Some(((*name).into(), std::env::var_os(name)?)))
                    .chain(
                        environment
                            .layout_variables()
                            .map(|(name, value)| (name.into(), value.clone())),
                    )
                    .collect(),
                start_files,
                ..session_settings(&config)
            },
            session_tick: Duration::from_secs(30),
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

/// The Claude Code adapter for the `claude` on the daemon's `PATH`, with the hook bridge and
/// the `cockpit` program installed beside the daemon. `None`, logged, when there is none
/// or it does not answer its version.
fn claude_code(daemon_program: &Path) -> Option<cockpit_claude::ClaudeCode> {
    let unavailable = |reason: &dyn std::fmt::Display| {
        tracing::warn!("provider claude unavailable: {reason}");
    };
    let Some(program) =
        std::env::var_os("PATH").and_then(|path| cockpit_claude::ClaudeCode::find_on_path(&path))
    else {
        unavailable(&"no `claude` on PATH");
        return None;
    };
    let bin_dir = daemon_program
        .parent()
        .expect("the daemon's program is a file in a directory");
    let bridge = cockpit_claude::Bridge {
        hook_program: bin_dir.join("cockpit-claude-hook"),
        cockpit_program: bin_dir.join("cockpit"),
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
