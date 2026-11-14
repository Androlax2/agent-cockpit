//! An approved run's egress (#1387): its systemd service has a network namespace of its own,
//! with nothing but loopback, and the hosts its card names go through a proxy of its own.
//!
//! - `torlek-sandbox run-egress FOLDER SESSION RUN HOST...`, started by the daemon on the
//!   host's network before the run: opens the hand-off socket in `FOLDER`, prints `ready`,
//!   takes the listener the run hands over and serves it with the card's hosts as the only
//!   entries (`cockpit_sandbox::card_allowlist`). It files no network request: the owner
//!   decided on the card. When its standard input closes, the run is over: it prints
//!   `refused DESTINATION` for each destination it refused and exits.
//! - `torlek-sandbox hand-over SOCKET PROGRAM ARGUMENT...`, the first program of the run's
//!   service: listens on the proxy address of the service's loopback, sends the listener to
//!   `run-egress` on `SOCKET` and runs `PROGRAM` in its place, keeping no copy.
//!
//! The hand-off is stage 2's (`handoff.rs`): the same socket, bytes and address check.

use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cockpit_sandbox::{Subject, card_allowlist};

use crate::{handoff, proxy};

/// The folder of `FOLDER` the hand-off socket is in: the run's service reaches it read-only.
pub const HANDOFF_FOLDER: &str = "proxy-handoff";

/// What `run-egress` prints once the run can hand its listener over.
pub const READY: &str = "ready";

pub fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let [folder, session, run, hosts @ ..] = arguments else {
        return Err("run-egress needs FOLDER SESSION RUN HOST...".to_owned());
    };
    if hosts.is_empty() {
        return Err("run-egress serves a card that names a host".to_owned());
    }
    let text = |argument: &OsString, what: &str| {
        argument
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("the {what} is not UTF-8: {argument:?}"))
    };
    let run: i64 = text(run, "run")?
        .parse()
        .map_err(|error| format!("the run is no number: {error}"))?;
    let hosts = hosts
        .iter()
        .map(|host| text(host, "host"))
        .collect::<Result<Vec<_>, _>>()?;
    let allowlist = card_allowlist(run, &hosts)?;
    let subject = Subject::Session {
        session: text(session, "session")?,
    };
    let waiting = handoff::Waiting::open(Path::new(folder).join(HANDOFF_FOLDER))?;
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{READY}")
        .and_then(|()| stdout.flush())
        .map_err(|error| format!("cannot say it is ready: {error}"))?;
    let is_over = Arc::new(AtomicBool::new(false));
    let watching = {
        let is_over = Arc::clone(&is_over);
        std::thread::Builder::new()
            .name("run-over".to_owned())
            .spawn(move || {
                let mut ignored = [0_u8; 64];
                while matches!(std::io::stdin().read(&mut ignored), Ok(read) if read > 0) {}
                is_over.store(true, Ordering::SeqCst);
            })
            .map_err(|error| format!("cannot watch for the run's end: {error}"))?
    };
    let received = waiting.receive_unless(&[], || Ok(is_over.load(Ordering::SeqCst)))?;
    let Some(received) = received else {
        return Ok(ExitCode::SUCCESS);
    };
    let proxy = proxy::Proxy::new(allowlist, subject, proxy::Network, None);
    let serving = Arc::clone(&proxy);
    std::thread::Builder::new()
        .name("egress-listener".to_owned())
        .spawn(move || serving.serve(received.proxy))
        .map_err(|error| format!("cannot start the run's proxy: {error}"))?;
    watching
        .join()
        .map_err(|_| "the watch for the run's end failed".to_owned())?;
    let report = proxy.report();
    let refused = report
        .destinations
        .iter()
        .filter(|(_, counters)| counters.refused > 0)
        .map(|(destination, _)| destination.as_str())
        .chain((report.other.refused > 0).then_some("other destinations"));
    for destination in refused {
        writeln!(stdout, "refused {destination}")
            .map_err(|error| format!("cannot name a refused destination: {error}"))?;
    }
    stdout
        .flush()
        .map_err(|error| format!("cannot name the refused destinations: {error}"))?;
    Ok(ExitCode::SUCCESS)
}

pub fn hand_over(arguments: &[OsString]) -> Result<std::convert::Infallible, String> {
    let [socket, program, program_arguments @ ..] = arguments else {
        return Err("hand-over needs SOCKET PROGRAM ARGUMENT...".to_owned());
    };
    // The connection and the listener close with this process image: the program never
    // holds them.
    let connection = handoff::hand_over(&PathBuf::from(socket), &[])?;
    drop(connection);
    let error = std::process::Command::new(program)
        .args(program_arguments)
        .exec();
    Err(format!(
        "cannot run {}: {error}",
        Path::new(program).display()
    ))
}
