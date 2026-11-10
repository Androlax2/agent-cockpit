//! `torlek-sandbox engine-relay SOCKET OWNER_UID` (#390): run by a project engine's relay
//! unit, which joins the engine's network namespace, so `127.0.0.1` here holds only the
//! ports that engine published and another project's engine is out of reach. The owner's
//! launcher connects to `SOCKET`, names one port in a line, reads `ok` or why not, then the
//! connection carries that port's bytes, unparsed.
//!
//! The request is the port in decimal and a newline; the answer is `ok\n`, or `refused: `,
//! the reason and a newline before the relay closes. Only `OWNER_UID` is served: whoever
//! else reaches the socket (the engine's own containers, through a bind) is closed at once.

use std::io::{ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

/// How long a client has to name its port, and the engine's port to accept.
const DEADLINE: Duration = Duration::from_secs(10);

/// The longest request: `65535\n`.
const MAX_REQUEST: usize = 6;

/// Serves `socket` until the process ends, one thread per connection.
pub fn run(socket: &Path, owner_uid: &std::ffi::OsStr) -> Result<std::convert::Infallible, String> {
    let owner_uid: u32 = owner_uid
        .to_str()
        .and_then(|text| text.parse().ok())
        .ok_or_else(|| format!("the owner's uid {owner_uid:?} is not a number"))?;
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
            .spawn(move || serve(client, owner_uid));
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

fn serve(mut client: UnixStream, owner_uid: u32) {
    match rustix::net::sockopt::socket_peercred(&client) {
        Ok(peer) if peer.uid.as_raw() == owner_uid => {}
        _ => {
            let _ = client.shutdown(Shutdown::Both);
            return;
        }
    }
    let target = read_port(&mut client).and_then(|port| {
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

/// The port the client names, read a byte at a time so none of the relayed bytes is taken.
fn read_port(client: &mut UnixStream) -> Result<u16, String> {
    client
        .set_read_timeout(Some(DEADLINE))
        .map_err(|error| format!("cannot set a deadline: {error}"))?;
    let mut request = Vec::with_capacity(MAX_REQUEST);
    let mut byte = [0_u8; 1];
    while request.last() != Some(&b'\n') {
        if request.len() == MAX_REQUEST {
            return Err("the request is not a port and a newline".to_owned());
        }
        client
            .read_exact(&mut byte)
            .map_err(|error| format!("no port named: {error}"))?;
        request.push(byte[0]);
    }
    parse_port(&request[..request.len() - 1])
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
