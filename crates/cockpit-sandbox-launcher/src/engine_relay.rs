//! `torlek-sandbox engine-relay SOCKET OWNER_UID [EGRESS_PORT]` (#390): run by a project
//! engine's relay unit, which joins the engine's network namespace, so `127.0.0.1` here holds
//! only the ports that engine published and another project's engine is out of reach. The
//! owner's launcher connects to `SOCKET`, names one port in a line, reads `ok` or why not,
//! then the connection carries that port's bytes, unparsed.
//!
//! The request is the port in decimal and a newline; the answer is `ok\n`, or `refused: `,
//! the reason and a newline before the relay closes. Only `OWNER_UID` is served: whoever
//! else reaches the socket (the engine's own containers, through a bind) is closed at once.
//!
//! The request `egress\n` asks for the engine's egress listener instead (part 6b): the relay
//! listens on `127.0.0.1:EGRESS_PORT` in the engine's namespace, where the engine's
//! containers reach it, and sends the listener over (`handoff::LISTENER_BYTE`) to the owner's
//! engine proxy (`engine_proxy.rs`), keeping no copy: the port is open exactly while that
//! proxy holds it, and a second proxy is refused until it lets go. The relay then holds the
//! connection until the proxy closes it, so the proxy sees the relay end with its engine.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

/// How long a client has to name its port, and the engine's port to accept.
const DEADLINE: Duration = Duration::from_secs(10);

/// The longest request: `egress\n`, one byte longer than `65535\n`.
const MAX_REQUEST: usize = 7;

/// The request for the engine's egress listener.
pub(crate) const EGRESS_REQUEST: &[u8] = b"egress";

/// What the owner's client asks of the relay.
#[derive(Debug, PartialEq, Eq)]
enum Request {
    /// The bytes of the engine's port.
    Port(u16),
    /// The engine's egress listener.
    Egress,
}

/// The launcher's side: asks the relay on `stream` for `port`, once the kernel says
/// `engine_uid` listens there. The stream then carries that port's bytes.
pub fn request(mut stream: UnixStream, engine_uid: u32, port: u16) -> std::io::Result<UnixStream> {
    check_listener(&stream, engine_uid)?;
    stream.set_read_timeout(Some(ANSWER_DEADLINE))?;
    stream.write_all(format!("{port}\n").as_bytes())?;
    let answer = read_line(&mut stream, MAX_ANSWER)?;
    let answer = String::from_utf8_lossy(&answer);
    if answer != "ok" {
        return Err(std::io::Error::other(format!(
            "the engine's relay answered: {answer}"
        )));
    }
    stream.set_read_timeout(None)?;
    Ok(stream)
}

/// Refuses a relay the kernel does not name `engine_uid` as listening on.
pub(crate) fn check_listener(stream: &UnixStream, engine_uid: u32) -> std::io::Result<()> {
    let listener = rustix::net::sockopt::socket_peercred(stream)?.uid.as_raw();
    if listener != engine_uid {
        return Err(std::io::Error::other(format!(
            "uid {listener} listens on the engine's relay, not the engine's uid {engine_uid}"
        )));
    }
    Ok(())
}

/// The longest answer the launcher reads from a relay.
pub(crate) const MAX_ANSWER: usize = 512;

/// How long the launcher waits for the relay's answer: longer than the relay waits for the
/// engine's port, so the relay's reason arrives.
pub(crate) const ANSWER_DEADLINE: Duration = Duration::from_secs(15);

/// One line of at most `most` bytes with its newline, read a byte at a time so none of the
/// relayed bytes after it is taken; without the newline. `InvalidData` when it is longer.
pub(crate) fn read_line(stream: &mut UnixStream, most: usize) -> std::io::Result<Vec<u8>> {
    let mut line = Vec::with_capacity(most);
    let mut byte = [0_u8; 1];
    while line.last() != Some(&b'\n') {
        if line.len() == most {
            return Err(std::io::Error::new(
                ErrorKind::InvalidData,
                format!("no line within {most} bytes"),
            ));
        }
        stream.read_exact(&mut byte)?;
        line.push(byte[0]);
    }
    line.pop();
    Ok(line)
}

/// Serves `socket` until the process ends, one thread per connection. Without `egress_port`,
/// as an earlier setup's unit runs it, the egress listener is refused.
pub fn run(
    socket: &Path,
    owner_uid: &std::ffi::OsStr,
    egress_port: Option<&std::ffi::OsStr>,
) -> Result<std::convert::Infallible, String> {
    let owner_uid: u32 = owner_uid
        .to_str()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| format!("the owner's uid {owner_uid:?} is not a number"))?;
    let egress_port = egress_port
        .map(|port| {
            port.to_str()
                .ok_or_else(|| format!("the egress port {port:?} is not a port"))
                .and_then(|port| parse_port(port.as_bytes()))
        })
        .transpose()?;
    let listener = listen(socket)?;
    loop {
        let client = match listener.accept() {
            Ok((client, _)) => client,
            Err(error) => {
                eprintln!("torlek-sandbox engine-relay: cannot accept: {error}");
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
        };
        let spawned = std::thread::Builder::new()
            .name("engine-relay".to_owned())
            .spawn(move || serve(client, owner_uid, egress_port));
        // Out of threads: the client's connection closes, which it sees at once.
        drop(spawned);
    }
}

/// Listens on `socket`, replacing the one a relay that stopped left behind.
fn listen(socket: &Path) -> Result<UnixListener, String> {
    match std::fs::remove_file(socket) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(format!("cannot remove {}: {error}", socket.display())),
    }
    UnixListener::bind(socket)
        .map_err(|error| format!("cannot listen on {}: {error}", socket.display()))
}

fn serve(mut client: UnixStream, owner_uid: u32, egress_port: Option<u16>) {
    match rustix::net::sockopt::socket_peercred(&client) {
        Ok(peer) if peer.uid.as_raw() == owner_uid => {}
        _ => {
            let _ = client.shutdown(Shutdown::Both);
            return;
        }
    }
    let port = match read_request(&mut client) {
        Ok(Request::Port(port)) => Ok(port),
        Ok(Request::Egress) => return serve_egress(client, egress_port),
        Err(reason) => Err(reason),
    };
    let target = port.and_then(|port| {
        TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], port)), DEADLINE)
            .map_err(|error| format!("127.0.0.1:{port} is unreachable: {error}"))
    });
    let target = match target {
        Ok(target) => target,
        Err(reason) => {
            let _ = client.write_all(format!("refused: {reason}\n").as_bytes());
            let _ = client.shutdown(Shutdown::Both);
            return;
        }
    };
    if client.write_all(b"ok\n").is_err() || client.set_read_timeout(None).is_err() {
        return;
    }
    splice(client, target);
}

fn refuse(mut client: UnixStream, reason: &str) {
    let _ = client.write_all(format!("refused: {reason}\n").as_bytes());
    let _ = client.shutdown(Shutdown::Both);
}

/// Sends the egress listener over `client`, then holds the connection until the proxy closes
/// it or the relay ends.
fn serve_egress(mut client: UnixStream, egress_port: Option<u16>) {
    let Some(port) = egress_port else {
        return refuse(
            client,
            "this relay serves no egress; run the engine's setup again (#390)",
        );
    };
    let listener = match TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))) {
        Ok(listener) => listener,
        Err(error) => {
            return refuse(
                client,
                &format!("cannot listen on 127.0.0.1:{port}: {error}"),
            );
        }
    };
    if crate::handoff::send_listener(&client, &listener).is_err() {
        return;
    }
    // The proxy holds the only copy: the port closes when it ends.
    drop(listener);
    if client.set_read_timeout(None).is_ok() {
        let _ = std::io::copy(&mut client, &mut std::io::sink());
    }
}

/// The port the client names, or the egress listener.
fn read_request(client: &mut UnixStream) -> Result<Request, String> {
    client
        .set_read_timeout(Some(DEADLINE))
        .map_err(|error| format!("cannot set a deadline: {error}"))?;
    let request = read_line(client, MAX_REQUEST).map_err(|error| match error.kind() {
        ErrorKind::InvalidData => "the request is not a port and a newline".to_owned(),
        _ => format!("no port named: {error}"),
    })?;
    if request == EGRESS_REQUEST {
        return Ok(Request::Egress);
    }
    parse_port(&request).map(Request::Port)
}

fn parse_port(digits: &[u8]) -> Result<u16, String> {
    let shown = String::from_utf8_lossy(digits);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(format!("{shown:?} is not a port"));
    }
    match shown.parse::<u16>() {
        Ok(0) | Err(_) => Err(format!("{shown} is not a port")),
        Ok(port) => Ok(port),
    }
}

/// Copies both ways until the engine's side ends, or each side does.
fn splice(client: UnixStream, target: TcpStream) {
    let (Ok(mut client_reader), Ok(mut target_reader)) = (client.try_clone(), target.try_clone())
    else {
        return;
    };
    let (mut client_writer, mut target_writer) = (client, target);
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let _ = std::io::copy(&mut client_reader, &mut target_writer);
            // A client may half-close and still read the answer.
            let _ = target_writer.shutdown(Shutdown::Write);
        });
        let _ = std::io::copy(&mut target_reader, &mut client_writer);
        // Once the engine's side ended, a client left open would hold its thread for good.
        let _ = client_writer.shutdown(Shutdown::Both);
        let _ = target_reader.shutdown(Shutdown::Both);
    });
}

#[cfg(test)]
mod tests {
    use super::parse_port;

    #[test]
    fn a_port_is_one_to_five_digits_from_1_to_65535() {
        assert_eq!(parse_port(b"8080"), Ok(8080));
        assert_eq!(parse_port(b"65535"), Ok(65535));
        for refused in [&b""[..], b"0", b"65536", b"80a", b"-1", b" 80", b"+80"] {
            assert!(parse_port(refused).is_err(), "{refused:?}");
        }
    }
}
