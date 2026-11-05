//! `torlek-sandbox`, the native sandbox launcher (#829, report #546).
//!
//! - `torlek-sandbox run SPEC`: reads and removes the daemon's launch spec, then starts its
//!   program under bubblewrap (new user, mount, PID, network, IPC, UTS and cgroup
//!   namespaces, a mount view of only what the spec names) through stage 2, serves the
//!   session proxy on the listener stage 2 hands it (#831) with the owner's decisions on
//!   its network requests (#674), and waits.
//! - `torlek-sandbox stage2 POLICY ARGUMENT...`: inside the namespaces, checks the folders the launcher
//!   pinned are the ones it saw, applies Landlock and the seccomp filter from memory, then
//!   runs the program in its place with the arguments. Never run by hand. Each argument
//!   travels on its own, so each keeps the kernel's per-argument limit it has unsandboxed.
//! - `torlek-sandbox probe BWRAP`: a short probe launch the daemon runs at its start; prints
//!   the bubblewrap version and the Landlock ABI the kernel gives a sandboxed process.

#[cfg(target_os = "linux")]
mod client_hello;
#[cfg(target_os = "linux")]
mod handoff;
#[cfg(target_os = "linux")]
mod plan;
#[cfg(target_os = "linux")]
mod probe;
#[cfg(target_os = "linux")]
mod proxy;
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

/// The folder of the session's mount folder where stage 2 hands the proxy listener over.
#[cfg(target_os = "linux")]
const HANDOFF_FOLDER: &str = "proxy-handoff";

/// Starts the spec's program in the sandbox, serves its proxy, and waits for it; its exit
/// status is the launcher's, a signal's as the shell reports it (128 + the signal).
#[cfg(target_os = "linux")]
fn run(spec_path: PathBuf) -> Result<ExitCode, String> {
    let spec = plan::read_spec(&spec_path)?;
    let mount_dir = spec_path
        .parent()
        .ok_or("the launch spec has no folder")?
        .to_path_buf();
    let launcher = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("cannot find the launcher's own program: {error}"))?;
    let waiting = handoff::Waiting::open(mount_dir.join(HANDOFF_FOLDER))?;
    let plan = plan::plan(&spec, &launcher, &waiting.socket())?;
    let mut child = std::process::Command::new(&spec.bubblewrap)
        .args(&plan.bubblewrap_arguments)
        .arg("--")
        .arg(&launcher)
        .arg("stage2")
        .arg(plan.policy_argument()?)
        .args(&spec.arguments)
        .spawn()
        .map_err(|error| {
            format!(
                "cannot start bubblewrap at {}: {error}",
                spec.bubblewrap.display()
            )
        })?;
    let listener = match waiting.receive(&mut child) {
        Ok(Some(listener)) => listener,
        // Stage 2 failed before the hand-off and said why.
        Ok(None) => return wait(&mut child),
        Err(reason) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(reason);
        }
    };
    let proxy = proxy::Proxy::new(spec.egress.clone(), spec.subject.clone(), proxy::Network);
    let report = mount_dir.join(cockpit_sandbox::EGRESS_FILE);
    proxy.keep_report(report.clone());
    // The owner's decisions (#674) apply before the first connection is served.
    let decisions = mount_dir.join(cockpit_sandbox::DECISIONS_FILE);
    proxy.load_decisions(&decisions);
    proxy.keep_decisions(decisions);
    // Only a session asks the owner; a check run's requests are #862's.
    if matches!(spec.subject, cockpit_sandbox::Subject::Session { .. }) {
        proxy.ask_through(mount_dir.join(cockpit_sandbox::REQUESTS_FILE));
    }
    let serving = std::sync::Arc::clone(&proxy);
    std::thread::Builder::new()
        .name("egress-listener".to_owned())
        .spawn(move || serving.serve(listener))
        .map_err(|error| format!("cannot start the session proxy: {error}"))?;
    let exit = wait(&mut child);
    // The last counters; the report thread may not have written them yet.
    let _ = proxy.write_report(&report);
    exit
}

#[cfg(target_os = "linux")]
fn wait(child: &mut std::process::Child) -> Result<ExitCode, String> {
    child
        .wait()
        .map(exit_code)
        .map_err(|error| format!("cannot wait for bubblewrap: {error}"))
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
