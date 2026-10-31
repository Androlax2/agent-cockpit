//! `cockpit-daemon`: this user's one Cockpit daemon (report 03). `--foreground` also prints
//! the log to stderr. It prints `ready <socket>` on stdout once it accepts requests.

use std::process::ExitCode;
use std::sync::Arc;

use cockpit_daemon::{Instance, Options, Routes, StartError};

const USAGE: &str = "usage: cockpit-daemon [--foreground]";

fn main() -> ExitCode {
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
    let options = Options::from_environment(&cockpit_config::Environment::from_process())?;
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
    let runtime = tokio::runtime::Runtime::new().map_err(StartError::Runtime)?;
    runtime.block_on(async {
        let daemon = instance.start(Arc::new(Routes)).await?;
        println!("ready {}", daemon.socket_path().display());
        daemon.wait().await;
        Ok(())
    })
}
