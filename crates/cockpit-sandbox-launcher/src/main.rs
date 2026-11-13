//! `torlek-sandbox`, the native sandbox launcher (#829, report #546).
//!
//! - `torlek-sandbox run SPEC`: reads and removes the daemon's launch spec, then starts its
//!   program under bubblewrap (new user, mount, PID, network, IPC, UTS and cgroup
//!   namespaces, a mount view of only what the spec names) through stage 2, serves the
//!   session proxy on the listener stage 2 hands it (#831) with the owner's decisions on
//!   its network requests (#674), relays each declared service the owner granted (#880),
//!   and waits.
//! - `torlek-sandbox stage2 POLICY ARGUMENT...`: inside the namespaces, checks the folders the launcher
//!   pinned are the ones it saw, applies Landlock and the seccomp filter from memory, then
//!   runs the program in its place with the arguments. Never run by hand. Each argument
//!   travels on its own, so each keeps the kernel's per-argument limit it has unsandboxed.
//! - `torlek-sandbox probe BWRAP`: a short probe launch the daemon runs at its start; prints
//!   the bubblewrap version and the Landlock ABI the kernel gives a sandboxed process.
//! - `torlek-sandbox engine-relay SOCKET OWNER_UID`: run by a project engine's relay unit
//!   inside the engine's network namespace (#390); relays the owner's connections to the
//!   ports that engine published.
//! - `torlek-sandbox engine-masks ENGINES_DIR`: run as root by the engine masks' unit for each
//!   connection (#390); shows a project's engine one checkout through the session baseline's
//!   masks.

#[cfg(target_os = "linux")]
mod authority;
#[cfg(target_os = "linux")]
mod client_hello;
#[cfg(target_os = "linux")]
mod engine_masks;
#[cfg(target_os = "linux")]
mod engine_relay;
#[cfg(target_os = "linux")]
mod handoff;
#[cfg(target_os = "linux")]
mod intercept;
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
        Some((verb, [])) if verb == handoff::BINDER_VERB => {
            stage2::detach_binder().map(|()| ExitCode::SUCCESS)
        }
        Some((verb, [serves]))
            if verb == handoff::BINDER_VERB && serves == handoff::BINDER_SERVES =>
        {
            // Stage 2 gave it the hand-off connection as its standard input.
            use std::os::fd::AsFd;
            std::io::stdin()
                .as_fd()
                .try_clone_to_owned()
                .map_err(|error| format!("cannot take the launcher's connection: {error}"))
                .map(std::os::unix::net::UnixStream::from)
                .and_then(|launcher| handoff::serve_binds(&launcher))
                .map(|()| ExitCode::SUCCESS)
        }
        Some((verb, [socket, owner_uid])) if verb == "engine-relay" => {
            engine_relay::run(std::path::Path::new(socket), owner_uid).map(|never| match never {})
        }
        Some((verb, rest)) if verb == "engine-masks" => engine_masks::engines_dir(rest)
            .and_then(|folder| engine_masks::serve(&folder))
            .map(|()| ExitCode::SUCCESS),
        _ => Err(
            "usage: torlek-sandbox run SPEC | torlek-sandbox probe BWRAP | torlek-sandbox \
             engine-relay SOCKET OWNER_UID | torlek-sandbox engine-masks ENGINES_DIR \
             (stage2 is internal)"
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
    let mount_dir = spec_path
        .parent()
        .ok_or("the launch spec has no folder")?
        .to_path_buf();
    let credentials = plan::take_credentials(&mount_dir)?;
    // A credential may be bound to a host the owner's decisions let through (#832, #674); the
    // proxy starts with the same decisions the binding was checked against.
    let decisions = mount_dir.join(cockpit_sandbox::DECISIONS_FILE);
    let launch_decisions = proxy::decisions_at(&decisions);
    let spec = plan::read_spec(&spec_path, &launch_decisions)?;
    // Before the program starts: the trust files it reads at its start are written.
    let values = plan::credential_values(credentials, &spec)?;
    let authority = match &spec.interception {
        Some(interception) => {
            let cockpit_sandbox::Subject::Session { session } = &spec.subject else {
                return Err("a check binds no credential, yet its spec intercepts".to_owned());
            };
            Some(authority::Authority::create(session, interception, values)?)
        }
        None => None,
    };
    let launcher = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("cannot find the launcher's own program: {error}"))?;
    let waiting = handoff::Waiting::open(mount_dir.join(HANDOFF_FOLDER))?;
    if let Some(engine) = &spec.engine {
        proxy::check_engine_socket(engine)?;
    }
    // Before the sandbox starts: bubblewrap binds the folders the sockets are in.
    let sockets = proxy::bind_sockets(&mount_dir, &spec.forwards)?;
    let plan = plan::plan(&spec, &launcher, &waiting.socket(), &sockets.folders)?;
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
    let forward_ports: Vec<u16> = plan
        .policy
        .forward_ports
        .iter()
        .map(|forward| forward.port)
        .collect();
    let received = match waiting.receive(&mut child, &forward_ports) {
        Ok(Some(received)) => received,
        // Stage 2 failed before the hand-off and said why.
        Ok(None) => return wait(&mut child),
        Err(reason) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(reason);
        }
    };
    let proxy = proxy::Proxy::new(
        spec.egress.clone(),
        spec.subject.clone(),
        proxy::Network,
        authority,
    );
    let report = mount_dir.join(cockpit_sandbox::EGRESS_FILE);
    proxy.keep_report(report.clone());
    // Before the decisions: they may open container ports live (#390).
    proxy.open_live_through(received.binder)?;
    // The owner's decisions (#674) apply before the first connection is served.
    proxy.apply_decisions(launch_decisions);
    proxy.keep_decisions(decisions);
    // A session and a check run both ask the owner (#352, #862 Q2).
    proxy.ask_through(mount_dir.join(cockpit_sandbox::REQUESTS_FILE));
    let serving = std::sync::Arc::clone(&proxy);
    let proxy_listener = received.proxy;
    std::thread::Builder::new()
        .name("egress-listener".to_owned())
        .spawn(move || serving.serve(proxy_listener))
        .map_err(|error| format!("cannot start the session proxy: {error}"))?;
    // Keyed by what each listener is bound to, so no forward is served on another's.
    let mut port_listeners: std::collections::HashMap<u16, std::net::TcpListener> = received
        .forwards
        .into_iter()
        .filter_map(|listener| Some((listener.local_addr().ok()?.port(), listener)))
        .collect();
    let mut socket_listeners: std::collections::HashMap<usize, _> =
        sockets.listeners.into_iter().collect();
    for (index, forward) in spec.forwards.iter().enumerate() {
        let listener = match forward.listen_port() {
            Some(port) => port_listeners.remove(&port).map(proxy::Listener::Port),
            None => socket_listeners.remove(&index).map(proxy::Listener::Socket),
        };
        let Some(listener) = listener else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("service {} has no listener", forward.service));
        };
        let serving = std::sync::Arc::clone(&proxy);
        let served = forward.clone();
        std::thread::Builder::new()
            .name("forward-listener".to_owned())
            .spawn(move || serving.serve_forward(served, listener))
            .map_err(|error| format!("cannot serve service {}: {error}", forward.service))?;
    }
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
