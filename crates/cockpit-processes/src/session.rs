//! The session leader of a terminal execution ([`crate::Terminal`]).
//!
//! A terminal becomes a process's controlling terminal only when a session leader takes it.
//! Making the child one between `fork` and `exec` would need `unsafe`, so a leader program
//! does it in its own process: it calls `setsid`, takes its standard input (the terminal) as
//! controlling terminal, then execs the rest of its command line, which keeps its pid.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::ExitCode;

/// The leader program's work: `arguments` is `<program> <arguments…>`. It becomes a session
/// leader, takes its standard input as controlling terminal and execs the program, which
/// keeps its pid, session and terminal. Returns only on failure.
pub fn run_session_leader(arguments: &[OsString]) -> ExitCode {
    let Some((program, program_arguments)) = arguments.split_first() else {
        eprintln!("cockpit: the session leader needs a program to run");
        return ExitCode::from(2);
    };
    if let Err(error) = rustix::process::setsid() {
        eprintln!("cockpit: cannot start a session for the command: {error}");
        return ExitCode::from(127);
    }
    if let Err(error) = rustix::process::ioctl_tiocsctty(std::io::stdin()) {
        eprintln!("cockpit: cannot take the terminal for the command: {error}");
        return ExitCode::from(127);
    }
    let error = std::process::Command::new(program)
        .args(program_arguments)
        .exec();
    eprintln!(
        "cockpit: cannot run {}: {error}",
        Path::new(program).display()
    );
    ExitCode::from(127)
}
