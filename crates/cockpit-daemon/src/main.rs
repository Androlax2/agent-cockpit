//! `cockpit-daemon`: this user's one Cockpit daemon (report 03). `--foreground` also prints
//! the log to stderr. It prints `ready <socket>` on stdout once it accepts requests.

use std::path::PathBuf;
use std::process::ExitCode;

use cockpit_daemon::{RUN_LEADER_FLAG, SESSION_TICK, Serve, StartError, own_program, serve};

const USAGE: &str = "usage: cockpit-daemon [--foreground]";

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
    serve(Serve {
        foreground,
        programs_dir: programs_dir()?,
        session_tick: SESSION_TICK,
        idle_sleep_after: None,
    })
}

/// The directory this program is installed in.
fn programs_dir() -> Result<PathBuf, StartError> {
    Ok(own_program()?
        .parent()
        .expect("the daemon's program is a file in a directory")
        .to_path_buf())
}
