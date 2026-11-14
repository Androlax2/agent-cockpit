//! `torlek-sandbox root-run` (#1387, the owner's option B): the root part of an approved run,
//! in a system unit with the floor of limits no request lowers
//! ([`cockpit_sandbox::RootFloor`]).
//!
//! The daemon runs it as `/usr/bin/sudo -A -k -- torlek-sandbox root-run`, so the owner types
//! their password for each one. Its standard input is a Unix socket of the daemon's, whose
//! peer must be the user sudo names in `SUDO_UID`. On it the daemon sends
//! [`ROOT_RUN_DESCRIPTORS_BYTE`] with the run's standard input, output and error, then the
//! [`RootRunRequest`] on one line. The helper answers one line each:
//!
//! - `refused: ` and why, before anything ran;
//! - `started UNIT`, once `systemd-run` is started (systemd may not have made the unit yet),
//!   and later `exited CODE`, `systemd-run`'s, which is the command's once it ran.
//!
//! A daemon that closes the socket while the command runs has ended the run: the helper stops
//! the unit, again until `systemd-run` ends, so a unit made after the first try stops too.
//! Paths the request names are resolved here, as root, before the floor is checked, so a link
//! cannot lead a write past it. The helper's own program and its folder are kept read-only,
//! wherever it is installed, and Torlek's folders under the owner's home are hidden.

use std::io::ErrorKind;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cockpit_sandbox::{
    MAX_ROOT_RUN_REQUEST_BYTES, ROOT_RUN_DESCRIPTORS_BYTE, ROOT_RUN_MIN_SYSTEMD, RootFloor,
    RootRunRequest, root_run_unit,
};

use crate::handoff;

/// Programs run as root, never searched for in a `PATH`.
const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const GETENT: &str = "/usr/bin/getent";

/// The root part's `PATH`: the system's programs, never the owner's.
const ROOT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin";

/// How long the daemon has to send its request once sudo started the helper.
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

/// The standard input, output and error a root part gets, in that order.
const RUN_DESCRIPTORS: usize = 3;

/// How often the helper looks whether `systemd-run` ended, and tries again to stop a unit
/// whose daemon went.
const WAIT_POLL: Duration = Duration::from_millis(100);
const STOP_RETRY: Duration = Duration::from_secs(1);

pub fn serve(arguments: &[std::ffi::OsString]) -> Result<ExitCode, String> {
    if !arguments.is_empty() {
        return Err("usage: sudo torlek-sandbox root-run".to_owned());
    }
    let owner_uid = sudo_uid()?;
    if effective_uid()? != 0 {
        return Err("root-run runs as root, through sudo".to_owned());
    }
    let daemon = daemon_connection(owner_uid)?;
    match start(&daemon, owner_uid) {
        Ok(child) => wait(&daemon, child),
        Err(reason) => {
            let _ = writeln!(&daemon, "refused: {reason}");
            Err(reason)
        }
    }
}

/// The owner sudo ran the helper for. Root is no owner: a root part is for an agent's run.
fn sudo_uid() -> Result<u32, String> {
    let text = std::env::var("SUDO_UID")
        .map_err(|_| "root-run runs through sudo, which sets SUDO_UID".to_owned())?;
    match text.parse::<u32>() {
        Ok(0) => Err("root-run serves an owner who is not root".to_owned()),
        Ok(uid) => Ok(uid),
        Err(error) => Err(format!("SUDO_UID {text:?} is no user id: {error}")),
    }
}

/// The effective user of this process, the second field of `Uid:` in `/proc/self/status`.
fn effective_uid() -> Result<u32, String> {
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|error| format!("cannot read /proc/self/status: {error}"))?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|ids| ids.split_whitespace().nth(1))
        .and_then(|effective| effective.parse().ok())
        .ok_or_else(|| "/proc/self/status names no effective user".to_owned())
}

/// The standard input sudo kept: the daemon's Unix socket, its peer the owner. sudo running
/// the command in a pseudo-terminal of its own (`use_pty` from a terminal) would hand a pipe
/// instead; that is refused, never served.
fn daemon_connection(owner_uid: u32) -> Result<UnixStream, String> {
    let input: OwnedFd = std::io::stdin()
        .as_fd()
        .try_clone_to_owned()
        .map_err(|error| format!("cannot take the standard input: {error}"))?;
    let status = rustix::fs::fstat(&input)
        .map_err(|error| format!("cannot read the standard input: {error}"))?;
    if rustix::fs::FileType::from_raw_mode(status.st_mode) != rustix::fs::FileType::Socket {
        return Err(
            "the standard input is not the daemon's socket: run root-run from the daemon, \
             without a terminal"
                .to_owned(),
        );
    }
    let peer = rustix::net::sockopt::socket_peercred(&input)
        .map_err(|error| format!("cannot tell who is on the standard input: {error}"))?;
    let peer_uid = peer.uid.as_raw();
    if peer_uid != owner_uid {
        return Err(format!(
            "the standard input's peer is user {peer_uid}, not {owner_uid}, whom sudo ran \
             root-run for"
        ));
    }
    Ok(UnixStream::from(input))
}

/// Reads the request, checks it against the floor and starts its unit.
fn start(daemon: &UnixStream, owner_uid: u32) -> Result<(std::process::Child, String), String> {
    daemon
        .set_read_timeout(Some(REQUEST_DEADLINE))
        .map_err(|error| format!("cannot wait for the request: {error}"))?;
    let (byte, descriptors) = handoff::receive(daemon, RUN_DESCRIPTORS)
        .map_err(|error| format!("cannot take the run's terminal: {error}"))?;
    if byte != ROOT_RUN_DESCRIPTORS_BYTE || descriptors.len() != RUN_DESCRIPTORS {
        return Err(format!(
            "the daemon sent {byte:?} and {} descriptors, not {ROOT_RUN_DESCRIPTORS_BYTE:?} and \
             the run's standard input, output and error",
            descriptors.len()
        ));
    }
    let request = resolved(read_request(daemon)?)?;
    systemd_holds_the_floor()?;
    let home = owner_home(owner_uid)?;
    let floor = RootFloor::for_owner(&home)
        .hiding(torlek_folders(&home))
        .keeping(own_install()?)
        .resolved(|path| std::fs::canonicalize(path).ok());
    let properties = floor.unit_properties(&request)?;
    let unit = root_run_unit(owner_uid, &request, std::process::id());
    let [input, output, error]: [OwnedFd; RUN_DESCRIPTORS] = descriptors
        .try_into()
        .map_err(|_| "counted above".to_owned())?;
    let child = Command::new(SYSTEMD_RUN)
        .args(systemd_run_arguments(&unit, &request, &properties))
        .env_clear()
        .stdin(Stdio::from(input))
        .stdout(Stdio::from(output))
        .stderr(Stdio::from(error))
        .spawn()
        .map_err(|error| format!("cannot start {SYSTEMD_RUN}: {error}"))?;
    Ok((child, unit))
}

fn read_request(daemon: &UnixStream) -> Result<RootRunRequest, String> {
    let mut line = Vec::new();
    let limit = u64::try_from(MAX_ROOT_RUN_REQUEST_BYTES).unwrap_or(u64::MAX);
    BufReader::new(daemon.take(limit))
        .read_until(b'\n', &mut line)
        .map_err(|error| format!("cannot read the request: {error}"))?;
    if line.pop() != Some(b'\n') {
        return Err(format!(
            "the request ended without its newline, or ran past {MAX_ROOT_RUN_REQUEST_BYTES} bytes"
        ));
    }
    serde_json::from_slice(&line).map_err(|error| format!("the request does not parse: {error}"))
}

/// `request` with its working folder and the folders it writes as root resolves them now, and
/// each folder it hides also as it resolves, if it does.
fn resolved(mut request: RootRunRequest) -> Result<RootRunRequest, String> {
    let resolve = |path: &Path| {
        std::fs::canonicalize(path)
            .map_err(|error| format!("cannot resolve {}: {error}", path.display()))
    };
    request.cwd = resolve(&request.cwd)?;
    request.writable = request
        .writable
        .iter()
        .map(|path| resolve(path))
        .collect::<Result<Vec<PathBuf>, String>>()?;
    let resolved_hidden: Vec<PathBuf> = request
        .hidden
        .iter()
        .filter_map(|path| {
            std::fs::canonicalize(path)
                .ok()
                .filter(|resolved| resolved != path)
        })
        .collect();
    request.hidden.extend(resolved_hidden);
    Ok(request)
}

/// Refuses a systemd too old for the floor's properties, before the owner's password is spent
/// on a unit that would not start.
fn systemd_holds_the_floor() -> Result<(), String> {
    let output = Command::new(SYSTEMCTL)
        .arg("--version")
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run {SYSTEMCTL}: {error}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text
        .split_whitespace()
        .nth(1)
        .and_then(|version| version.parse::<u32>().ok())
        .ok_or_else(|| format!("cannot tell systemd's version from {text:?}"))?;
    if version < ROOT_RUN_MIN_SYSTEMD {
        return Err(format!(
            "systemd {version} is older than {ROOT_RUN_MIN_SYSTEMD}, which a root part's unit \
             needs (PrivatePIDs=)"
        ));
    }
    Ok(())
}

/// Torlek's folders under `home` a profile made: each `torlek*` in its configuration and state
/// folders, which the floor hides as it hides the default ones.
fn torlek_folders(home: &Path) -> Vec<PathBuf> {
    [".config", ".local/state"]
        .iter()
        .filter_map(|base| std::fs::read_dir(home.join(base)).ok())
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().as_encoded_bytes().starts_with(b"torlek"))
        .map(|entry| entry.path())
        .collect()
}

/// This program and its folder, kept read-only so no root part replaces the floor.
fn own_install() -> Result<Vec<PathBuf>, String> {
    let program = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|error| format!("cannot find root-run's own program: {error}"))?;
    let folder = program
        .parent()
        .map(Path::to_path_buf)
        .ok_or("root-run's program has no folder")?;
    Ok(vec![program, folder])
}

/// The home the system's user database gives `uid`, its own and not the daemon's word.
fn owner_home(uid: u32) -> Result<PathBuf, String> {
    let output = Command::new(GETENT)
        .args(["passwd", &uid.to_string()])
        .env_clear()
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|error| format!("cannot run {GETENT}: {error}"))?;
    if !output.status.success() {
        return Err(format!("the user database has no user {uid}"));
    }
    let entry =
        String::from_utf8(output.stdout).map_err(|_| format!("user {uid}'s entry is not UTF-8"))?;
    let home = entry
        .lines()
        .next()
        .and_then(|line| line.split(':').nth(5))
        .map(PathBuf::from)
        .filter(|home| home.is_absolute() && home != Path::new("/"))
        .ok_or_else(|| format!("user {uid} has no home of their own: {entry:?}"))?;
    Ok(home)
}

/// `systemd-run`'s arguments for `request` in `unit`: it waits, passes the run's descriptors
/// through, expands nothing in the command and collects the unit when it ends.
fn systemd_run_arguments(
    unit: &str,
    request: &RootRunRequest,
    properties: &[String],
) -> Vec<String> {
    let mut arguments: Vec<String> = [
        "--system",
        "--quiet",
        "--pipe",
        "--wait",
        "--collect",
        "--expand-environment=no",
        "--service-type=exec",
    ]
    .map(str::to_owned)
    .into();
    arguments.push(format!("--unit={unit}"));
    arguments.push(format!(
        "--description=Torlek: the root part of approved run {}",
        request.run
    ));
    arguments.push(format!("--working-directory={}", request.cwd.display()));
    arguments.push(format!("--setenv=PATH={ROOT_PATH}"));
    arguments.push("--setenv=HOME=/root".to_owned());
    if let Some(term) = &request.term {
        arguments.push(format!("--setenv=TERM={term}"));
    }
    arguments.extend(
        properties
            .iter()
            .map(|property| format!("--property={property}")),
    );
    arguments.push("--".to_owned());
    arguments.extend(request.argv.iter().cloned());
    arguments
}

/// Waits for the unit, stopping it if the daemon goes, and tells the daemon its exit code.
fn wait(
    daemon: &UnixStream,
    (mut child, unit): (std::process::Child, String),
) -> Result<ExitCode, String> {
    // A daemon gone already is seen below: its socket's end stops the unit.
    let _ = writeln!(&*daemon, "started {unit}");
    let daemon_went = watch_for_the_end(daemon);
    let mut last_stop: Option<Instant> = None;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("cannot wait for {SYSTEMD_RUN}: {error}"))?
        {
            break status;
        }
        let is_stop_due = last_stop.is_none_or(|stopped| stopped.elapsed() >= STOP_RETRY);
        if daemon_went.load(Ordering::Acquire) && is_stop_due {
            stop(&unit);
            last_stop = Some(Instant::now());
        }
        std::thread::sleep(WAIT_POLL);
    };
    let code = status.code().unwrap_or_else(|| {
        use std::os::unix::process::ExitStatusExt;
        128 + status.signal().unwrap_or(0)
    });
    writeln!(&*daemon, "exited {code}")
        .map_err(|error| format!("cannot tell the daemon the exit code {code}: {error}"))?;
    Ok(ExitCode::from(u8::try_from(code).unwrap_or(u8::MAX)))
}

/// Set once the daemon's side of the socket ends. Whatever the daemon sends now is noise; a
/// read that was only interrupted or timed out reads again.
fn watch_for_the_end(daemon: &UnixStream) -> Arc<AtomicBool> {
    let went = Arc::new(AtomicBool::new(false));
    let watching = match daemon.try_clone() {
        Ok(watching) => watching,
        Err(error) => {
            eprintln!(
                "torlek-sandbox: cannot watch the daemon's socket ({error}); a daemon that goes \
                 will not stop the unit"
            );
            return went;
        }
    };
    let _ = watching.set_read_timeout(None);
    let setting = Arc::clone(&went);
    std::thread::spawn(move || {
        let mut rest = [0_u8; 64];
        loop {
            match (&watching).read(&mut rest) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::Interrupted | ErrorKind::WouldBlock | ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
        setting.store(true, Ordering::Release);
    });
    went
}

/// Stops `unit`, which a run the daemon ended leaves behind.
fn stop(unit: &str) {
    let stopped = Command::new(SYSTEMCTL)
        .args(["stop", "--no-block", unit])
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status();
    match stopped {
        Ok(status) if status.success() => {}
        other => eprintln!("torlek-sandbox: cannot stop {unit}: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_follows_its_properties_and_nothing_in_it_is_expanded() {
        let request = RootRunRequest {
            run: 9,
            ask: 2,
            argv: vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                "echo $HOME %h".to_owned(),
            ],
            cwd: PathBuf::from("/srv/shop"),
            writable: Vec::new(),
            hidden: Vec::new(),
            term: Some("xterm".to_owned()),
        };
        let arguments = systemd_run_arguments(
            "torlek-root-run-1-9-2-3.service",
            &request,
            &["PrivateNetwork=yes".to_owned()],
        );
        assert!(arguments.contains(&"--expand-environment=no".to_owned()));
        assert!(arguments.contains(&"--property=PrivateNetwork=yes".to_owned()));
        assert!(arguments.contains(&"--setenv=TERM=xterm".to_owned()));
        let command_start = arguments
            .iter()
            .position(|argument| argument == "--")
            .unwrap();
        assert_eq!(arguments[command_start + 1..], request.argv[..]);
    }
}
