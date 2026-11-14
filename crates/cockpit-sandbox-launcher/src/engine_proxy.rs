//! `torlek-sandbox engine-proxy RELAY_SOCKET ENGINE_UID PROJECT FOLDER EGRESS_PORT` (#390
//! part 6b): the egress proxy of one project's engine, for the owner's daemon to run (part 6c;
//! no daemon runs it yet). It asks the engine's relay (`engine_relay.rs`) for the listener on
//! `127.0.0.1:EGRESS_PORT` in the engine's network namespace, where dockerd's pulls reach it,
//! and containers told it is their proxy (part 6d), and serves it as a session's proxy is served, for the subject "the containers of
//! PROJECT": only what the owner allowed always for the project passes (#856 Q4 A), and an
//! unknown host is refused at once and asked of the owner.
//!
//! `FOLDER` holds the files the daemon and the proxy share, as a session's mount folder
//! does: the owner's decisions (`cockpit_sandbox::DECISIONS_FILE`), the requests
//! (`REQUESTS_FILE`) and the counters (`EGRESS_FILE`). The proxy ends when the relay's
//! connection ends, so a stopped engine leaves no proxy, and the listener closes with the
//! proxy, so containers have no egress while the daemon is stopped.

use std::ffi::OsStr;
use std::net::{SocketAddr, TcpListener};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;

use crate::engine_relay;
use crate::handoff;
use crate::proxy;

/// What the daemon's command line names.
struct Arguments {
    relay_socket: PathBuf,
    engine_uid: u32,
    project: String,
    folder: PathBuf,
    egress_port: u16,
}

/// Serves the engine's egress listener until the relay's connection ends; that end is an
/// error, as the daemon restarts the proxy only while the engine runs.
pub fn run(arguments: &[std::ffi::OsString]) -> Result<std::convert::Infallible, String> {
    let arguments = parse(arguments)?;
    let mut relay = UnixStream::connect(&arguments.relay_socket).map_err(|error| {
        format!(
            "cannot reach the engine's relay at {}: {error}",
            arguments.relay_socket.display()
        )
    })?;
    engine_relay::check_listener(&relay, arguments.engine_uid)
        .map_err(|error| format!("refusing the engine's relay: {error}"))?;
    let listener = egress_listener(&mut relay, arguments.egress_port)?;
    let subject = cockpit_sandbox::Subject::Containers {
        project: arguments.project.clone(),
    };
    let allowlist = cockpit_sandbox::Allowlist {
        program: format!("containers of project {}", arguments.project),
        entries: Vec::new(),
    };
    let proxy = proxy::Proxy::new(allowlist, subject, proxy::Network, None);
    let report = arguments.folder.join(cockpit_sandbox::EGRESS_FILE);
    proxy.keep_report(report.clone());
    let decisions = arguments.folder.join(cockpit_sandbox::DECISIONS_FILE);
    proxy.apply_decisions(proxy::decisions_at(&decisions));
    proxy.keep_decisions(decisions);
    proxy.ask_through(arguments.folder.join(cockpit_sandbox::REQUESTS_FILE));
    let serving = Arc::clone(&proxy);
    std::thread::Builder::new()
        .name("egress-listener".to_owned())
        .spawn(move || serving.serve(listener))
        .map_err(|error| format!("cannot start the engine's proxy: {error}"))?;
    let held = std::io::copy(&mut relay, &mut std::io::sink());
    // The last counters; the report thread may not have written them yet.
    let _ = proxy.write_report(&report);
    Err(match held {
        Ok(_) => "the engine's relay closed: the engine stopped".to_owned(),
        Err(error) => format!("the engine's relay failed: {error}"),
    })
}

fn parse(arguments: &[std::ffi::OsString]) -> Result<Arguments, String> {
    let [relay_socket, engine_uid, project, folder, egress_port] = arguments else {
        return Err(
            "usage: torlek-sandbox engine-proxy RELAY_SOCKET ENGINE_UID PROJECT FOLDER \
             EGRESS_PORT"
                .to_owned(),
        );
    };
    let engine_uid = utf8(engine_uid, "the engine's uid")?
        .parse()
        .map_err(|_| format!("the engine's uid {engine_uid:?} is not a number"))?;
    let project = utf8(project, "the project")?.to_owned();
    if project.is_empty() {
        return Err("the project is empty".to_owned());
    }
    let egress_port = match utf8(egress_port, "the egress port")?.parse::<u16>() {
        Ok(port) if port != 0 => port,
        _ => return Err(format!("the egress port {egress_port:?} is not a port")),
    };
    let folder = PathBuf::from(folder);
    // Before the listener is taken: a proxy that could file no request holds no port.
    if !folder.is_dir() {
        return Err(format!(
            "the proxy's folder {} is no folder",
            folder.display()
        ));
    }
    Ok(Arguments {
        relay_socket: PathBuf::from(relay_socket),
        engine_uid,
        project,
        folder,
        egress_port,
    })
}

fn utf8<'a>(argument: &'a OsStr, role: &str) -> Result<&'a str, String> {
    argument
        .to_str()
        .ok_or_else(|| format!("{role} {argument:?} is not UTF-8"))
}

/// Asks the relay for the listener on `127.0.0.1:egress_port`, or says why the relay refused.
fn egress_listener(relay: &mut UnixStream, egress_port: u16) -> Result<TcpListener, String> {
    use std::io::Write;
    let failed = |error: std::io::Error| format!("cannot ask the engine's relay: {error}");
    relay
        .set_read_timeout(Some(engine_relay::ANSWER_DEADLINE))
        .map_err(failed)?;
    relay
        .write_all(&[engine_relay::EGRESS_REQUEST, b"\n"].concat())
        .map_err(failed)?;
    let (byte, mut descriptors) = handoff::receive(relay, 1).map_err(failed)?;
    if byte != handoff::LISTENER_BYTE {
        let answer = engine_relay::read_line(relay, engine_relay::MAX_ANSWER)
            .map(|rest| String::from_utf8_lossy(&[&[byte][..], &rest].concat()).into_owned())
            .map_err(|error| {
                format!("the engine's relay refused and its answer failed: {error}")
            })?;
        return Err(format!("the engine's relay answered: {answer}"));
    }
    let Some(listener) = descriptors.pop() else {
        return Err("the engine's relay sent no listener".to_owned());
    };
    let listener = TcpListener::from(listener);
    let expected = SocketAddr::from(([127, 0, 0, 1], egress_port));
    match listener.local_addr() {
        Ok(address) if address == expected => {}
        bound => {
            return Err(format!(
                "the engine's relay sent a listener on {bound:?}, not {expected}"
            ));
        }
    }
    relay.set_read_timeout(None).map_err(failed)?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments(values: &[&str]) -> Vec<std::ffi::OsString> {
        values.iter().map(Into::into).collect()
    }

    #[test]
    fn a_port_of_zero_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let folder = folder.path().to_str().unwrap();
        let refused = parse(&arguments(&["relay.sock", "1000", "app", folder, "0"]));
        assert!(refused.is_err_and(|reason| reason.contains("is not a port")));
    }

    #[test]
    fn an_empty_project_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let folder = folder.path().to_str().unwrap();
        let refused = parse(&arguments(&["relay.sock", "1000", "", folder, "3128"]));
        assert!(refused.is_err_and(|reason| reason == "the project is empty"));
    }
}
