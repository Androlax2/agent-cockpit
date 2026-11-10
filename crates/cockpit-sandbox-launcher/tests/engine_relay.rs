//! `torlek-sandbox engine-relay` (#390), the real binary: the owner names a port and gets
//! that port's bytes, or why not. Here the relay shares this test's network namespace; in an
//! engine's relay unit it shares only the engine's, which holds nothing but its published
//! ports. This test's user stands for the owner.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_torlek-sandbox");

struct Relay {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    child: Child,
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn own_uid(dir: &Path) -> u32 {
    std::fs::metadata(dir).unwrap().uid()
}

/// A relay serving `owner_uid`, once its socket answers. `prepare` runs on the socket's
/// path before the relay starts.
fn relay_for(owner_uid: impl Fn(&Path) -> u32, prepare: impl FnOnce(&Path)) -> Relay {
    // A socket path must fit the kernel's 108 bytes, which the target folder may not.
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("relay.sock");
    prepare(&socket);
    let child = Command::new(LAUNCHER)
        .arg("engine-relay")
        .arg(&socket)
        .arg(owner_uid(dir.path()).to_string())
        .spawn()
        .unwrap();
    let relay = Relay {
        _dir: dir,
        socket,
        child,
    };
    let started = Instant::now();
    while UnixStream::connect(&relay.socket).is_err() {
        assert!(started.elapsed() < Duration::from_secs(10), "no relay");
        std::thread::sleep(Duration::from_millis(20));
    }
    relay
}

fn relay() -> Relay {
    relay_for(own_uid, |_| {})
}

/// Sends `request` and reads the answer line.
fn ask(relay: &Relay, request: &[u8]) -> (String, BufReader<UnixStream>) {
    let mut stream = UnixStream::connect(&relay.socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request).unwrap();
    let mut reader = BufReader::new(stream);
    let mut answer = String::new();
    reader.read_line(&mut answer).unwrap();
    (answer, reader)
}

/// A service on a loopback port that answers each line with `echo: ` and the line.
fn echo_service() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap() > 0 {
                stream
                    .write_all(format!("echo: {line}").as_bytes())
                    .unwrap();
                line.clear();
            }
        }
    });
    port
}

#[test]
fn the_named_port_gets_the_owners_bytes_and_answers_them() {
    let relay = relay();
    let port = echo_service();

    let (answer, mut reader) = ask(&relay, format!("{port}\n").as_bytes());
    reader.get_mut().write_all(b"hello\n").unwrap();
    let mut echoed = String::new();
    reader.read_line(&mut echoed).unwrap();

    assert_eq!(answer, "ok\n");
    assert_eq!(echoed, "echo: hello\n");
}

#[test]
fn bytes_sent_with_the_request_reach_the_port() {
    let relay = relay();
    let port = echo_service();

    let (answer, mut reader) = ask(&relay, format!("{port}\nhello\n").as_bytes());
    let mut echoed = String::new();
    reader.read_line(&mut echoed).unwrap();

    assert_eq!(answer, "ok\n");
    assert_eq!(echoed, "echo: hello\n");
}

#[test]
fn a_port_nothing_listens_on_is_refused_naming_it() {
    let relay = relay();
    let port = {
        let unused = TcpListener::bind("127.0.0.1:0").unwrap();
        unused.local_addr().unwrap().port()
    };

    let (answer, mut reader) = ask(&relay, format!("{port}\n").as_bytes());

    assert!(
        answer.starts_with(&format!("refused: 127.0.0.1:{port} is unreachable")),
        "{answer}"
    );
    assert_eq!(reader.read(&mut [0_u8; 8]).unwrap(), 0);
}

#[test]
fn a_request_that_is_not_a_port_is_refused() {
    let relay = relay();

    for request in [&b"0\n"[..], b"65536\n", b"http\n", b"8080 9090\n", b"\n"] {
        let (answer, _) = ask(&relay, request);

        assert!(answer.starts_with("refused: "), "{request:?}: {answer}");
    }
}

#[test]
fn anyone_but_the_owner_is_closed_without_an_answer() {
    let relay = relay_for(|dir| own_uid(dir) + 1, |_| {});
    let port = echo_service();

    let mut stream = UnixStream::connect(&relay.socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let _ = stream.write_all(format!("{port}\n").as_bytes());
    let mut answer = Vec::new();

    // The relay closes before reading the request, so the close may arrive as a reset.
    let read = stream.read_to_end(&mut answer);

    assert!(
        matches!(&read, Ok(0))
            || matches!(&read, Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset),
        "{read:?}: {answer:?}"
    );
}

#[test]
fn a_socket_left_by_a_stopped_relay_is_replaced() {
    let relay = relay_for(own_uid, |socket| {
        drop(std::os::unix::net::UnixListener::bind(socket).unwrap());
    });
    let port = echo_service();

    let (answer, reader) = ask(&relay, format!("{port}\n").as_bytes());

    assert_eq!(answer, "ok\n");
    reader.get_ref().shutdown(Shutdown::Both).unwrap();
}
