//! `torlek-sandbox`, the native sandbox launcher (#829, report #546).
//!
//! - `torlek-sandbox run SPEC`: reads and removes the daemon's launch spec, then starts its
//!   program under bubblewrap (new user, mount, PID, network, IPC, UTS and cgroup
//!   namespaces, a mount view of only what the spec names) through stage 2, and waits.
//! - `torlek-sandbox stage2 POLICY ARGUMENT...`: inside the namespaces, checks the folders the launcher
//!   pinned are the ones it saw, applies Landlock and the seccomp filter from memory, then
//!   runs the program in its place with the arguments. Never run by hand. Each argument
//!   travels on its own, so each keeps the kernel's per-argument limit it has unsandboxed.
//! - `torlek-sandbox probe BWRAP`: a short probe launch the daemon runs at its start; prints
//!   the bubblewrap version and the Landlock ABI the kernel gives a sandboxed process.

#[cfg(target_os = "linux")]
mod plan;
#[cfg(target_os = "linux")]
mod probe;
#[cfg(target_os = "linux")]
mod stage2;

#[cfg(target_os = "linux")]
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::process::ExitCode;

/// The exit status when the launcher itself refuses or fails, before the program runs; the
/// program's own statuses pass through unchanged.
const LAUNCHER_FAILED: u8 = 125;

#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    eprintln!("torlek-sandbox: the native sandbox needs Linux");
    ExitCode::from(LAUNCHER_FAILED)
}

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    let outcome = match arguments.split_first() {
        Some((verb, rest)) if verb == "run" && rest.len() == 1 => run(PathBuf::from(&rest[0])),
        Some((verb, [policy, program_arguments @ ..])) if verb == "stage2" => {
            stage2::run(policy, program_arguments).map(|never| match never {})
        }
        Some((verb, rest)) if verb == "probe" && rest.len() == 1 => {
            probe::run(PathBuf::from(&rest[0])).map(|()| ExitCode::SUCCESS)
        }
        Some((verb, [])) if verb == stage2::PROBE_VERB => {
            stage2::probe().map(|()| ExitCode::SUCCESS)
        }
        _ => Err(
            "usage: torlek-sandbox run SPEC | torlek-sandbox probe BWRAP (stage2 is internal)"
                .to_owned(),
        ),
    };
    outcome.unwrap_or_else(|reason| {
        eprintln!("torlek-sandbox: {reason}");
        ExitCode::from(LAUNCHER_FAILED)
    })
}

/// Starts the spec's program in the sandbox and waits for it; its exit status is the
/// launcher's, a signal's as the shell reports it (128 + the signal).
#[cfg(target_os = "linux")]
fn run(spec_path: PathBuf) -> Result<ExitCode, String> {
    let spec = plan::read_spec(&spec_path)?;
    let launcher = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("cannot find the launcher's own program: {error}"))?;
    let plan = plan::plan(&spec, &launcher)?;
    let status = std::process::Command::new(&spec.bubblewrap)
        .args(&plan.bubblewrap_arguments)
        .arg("--")
        .arg(&launcher)
        .arg("stage2")
        .arg(plan.policy_argument()?)
        .args(&spec.arguments)
        .status()
        .map_err(|error| {
            format!(
                "cannot start bubblewrap at {}: {error}",
                spec.bubblewrap.display()
            )
        })?;
    Ok(exit_code(status))
}

#[cfg(target_os = "linux")]
fn exit_code(status: std::process::ExitStatus) -> ExitCode {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(code), _) => ExitCode::from(u8::try_from(code).unwrap_or(LAUNCHER_FAILED)),
        (None, Some(signal)) => {
            ExitCode::from(u8::try_from(128 + signal).unwrap_or(LAUNCHER_FAILED))
        }
        (None, None) => ExitCode::from(LAUNCHER_FAILED),
    }
}
