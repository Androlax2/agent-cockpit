//! The end-to-end tests' daemon (#300): the product daemon with shorter clocks, so a test
//! sees a session fall asleep in seconds without lowering the configuration's floor.
//! Never installed: Cargo builds examples only on request.
//!
//!     cockpit-e2e-daemon --idle-sleep-after-ms N --session-tick-ms N
//!
//! It runs from `<profile>/examples/`; the installation it serves (`cockpit`,
//! `cockpit-claude-hook`) is the profile directory above it.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use cockpit_daemon::{RUN_LEADER_FLAG, Serve, StartError, serve};

const USAGE: &str = "usage: cockpit-e2e-daemon --idle-sleep-after-ms N --session-tick-ms N";

fn main() -> ExitCode {
    let mut raw_arguments = std::env::args_os().skip(1);
    // Approved runs start this program as their session leader, as they do the daemon.
    if raw_arguments.next().as_deref() == Some(RUN_LEADER_FLAG.as_ref()) {
        return cockpit_processes::run_session_leader(&raw_arguments.collect::<Vec<_>>());
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let words: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let (idle_sleep_after, session_tick) = match words.as_slice() {
        ["--idle-sleep-after-ms", idle, "--session-tick-ms", tick] => {
            match (milliseconds(idle), milliseconds(tick)) {
                (Some(idle), Some(tick)) => (idle, tick),
                _ => return usage(),
            }
        }
        _ => return usage(),
    };
    match run(idle_sleep_after, session_tick) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cockpit-e2e-daemon: {error}");
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn milliseconds(word: &str) -> Option<Duration> {
    word.parse()
        .ok()
        .filter(|count| *count > 0)
        .map(Duration::from_millis)
}

fn run(idle_sleep_after: Duration, session_tick: Duration) -> Result<(), StartError> {
    let program = std::env::current_exe().map_err(|source| StartError::Io {
        action: "find the daemon's own program",
        path: "/proc/self/exe".into(),
        source,
    })?;
    let programs_dir: PathBuf = program
        .ancestors()
        .nth(2)
        .expect("Cargo builds examples in <profile>/examples/")
        .to_path_buf();
    serve(Serve {
        foreground: false,
        programs_dir,
        session_tick,
        idle_sleep_after: Some(idle_sleep_after),
    })
}
