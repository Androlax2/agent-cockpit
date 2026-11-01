//! `cockpit-daemon`: this user's one Cockpit daemon per profile (report 03, #196).
//! `--foreground` also prints the log to stderr; `--profile NAME` selects the profile, as
//! `$COCKPIT_PROFILE` does. It prints `ready <socket>` on stdout once it accepts requests.

use std::path::PathBuf;
use std::process::ExitCode;

use cockpit_daemon::{
    RUN_LEADER_FLAG, SESSION_TICK, Serve, StartError, own_profile, own_program, serve,
};

const USAGE: &str = "usage: cockpit-daemon [--foreground] [--profile NAME]";

fn main() -> ExitCode {
    let mut raw_arguments = std::env::args_os().skip(1);
    // The daemon starts itself in this mode as the session leader of each approved run.
    if raw_arguments.next().as_deref() == Some(RUN_LEADER_FLAG.as_ref()) {
        return cockpit_processes::run_session_leader(&raw_arguments.collect::<Vec<_>>());
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(arguments) = parse(&arguments) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match run(arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("cockpit-daemon: {error}");
            ExitCode::FAILURE
        }
    }
}

struct Arguments<'a> {
    foreground: bool,
    profile: Option<&'a str>,
}

/// Each flag at most once, in any order.
fn parse(words: &[String]) -> Option<Arguments<'_>> {
    let mut arguments = Arguments {
        foreground: false,
        profile: None,
    };
    let mut words = words.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--foreground" if !arguments.foreground => arguments.foreground = true,
            "--profile" if arguments.profile.is_none() => {
                arguments.profile = Some(words.next()?.as_str());
            }
            _ => return None,
        }
    }
    Some(arguments)
}

fn run(arguments: Arguments<'_>) -> Result<(), StartError> {
    serve(Serve {
        profile: own_profile(arguments.profile)?,
        foreground: arguments.foreground,
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
