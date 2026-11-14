//! `torlek-sandbox engine-proxy` (#390 part 6b), the real binaries: the proxy of a project's
//! engine takes the egress listener its relay binds in the engine's network namespace and
//! serves it from the owner's side with the project's decisions, filing what it refuses as
//! the project's containers' requests. Here relay and proxy share this test's network
//! namespace and user, which stands for both the owner and the engine's user.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, TcpListener, TcpStream};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};
use std::time::{Duration, Instant};

use cockpit_sandbox::{AllowEntry, AllowSource, EgressDecisions, EgressRequests, Subject, Target};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_torlek-sandbox");

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct Engine {
    dir: tempfile::TempDir,
    relay: Running,
    port: u16,
}

impl Engine {
    fn socket(&self) -> PathBuf {
        self.dir.path().join("relay.sock")
    }

    fn folder(&self) -> PathBuf {
        self.dir.path().join("proxy")
    }

    fn uid(&self) -> u32 {
        std::fs::metadata(self.dir.path()).unwrap().uid()
    }

    /// Starts the proxy for project `app`, as the daemon does, as `engine_uid`.
    fn proxy_as(&self, engine_uid: u32) -> Running {
        Running(
            Command::new(LAUNCHER)
                .arg("engine-proxy")
                .arg(self.socket())
                .arg(engine_uid.to_string())
                .arg("app")
                .arg(self.folder())
                .arg(self.port.to_string())
                .spawn()
                .unwrap(),
        )
    }

    fn proxy(&self) -> Running {
        self.proxy_as(self.uid())
    }

    fn decide(&self, decisions: &EgressDecisions) {
        let path = self.folder().join(cockpit_sandbox::DECISIONS_FILE);
        let staged = path.with_extension("tmp");
        std::fs::write(&staged, serde_json::to_vec(decisions).unwrap()).unwrap();
        std::fs::rename(staged, path).unwrap();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// An engine's relay, with an egress port, once its socket answers; the proxy's folder made.
fn engine() -> Engine {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("proxy")).unwrap();
    let port = free_port();
    let owner = std::fs::metadata(dir.path()).unwrap().uid();
    let relay = Running(
        Command::new(LAUNCHER)
            .arg("engine-relay")
            .arg(dir.path().join("relay.sock"))
            .arg(owner.to_string())
            .arg(port.to_string())
            .spawn()
            .unwrap(),
    );
    let started = Instant::now();
    while UnixStream::connect(dir.path().join("relay.sock")).is_err() {
        assert!(started.elapsed() < Duration::from_secs(10), "no relay");
        std::thread::sleep(Duration::from_millis(20));
    }
    Engine { dir, relay, port }
}

/// A connection to the engine's egress port, once something serves it.
fn connect(engine: &Engine) -> TcpStream {
    let started = Instant::now();
    loop {
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", engine.port)) {
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            return stream;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "no proxy");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Sends `CONNECT host:port` and reads the status line.
fn tunnel(engine: &Engine, destination: &str) -> (String, BufReader<TcpStream>) {
    let mut stream = connect(engine);
    stream
        .write_all(
            format!("CONNECT {destination} HTTP/1.1\r\nHost: {destination}\r\n\r\n").as_bytes(),
        )
        .unwrap();
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).unwrap();
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap() > 2 {
        line.clear();
    }
    (status, reader)
}

fn wait_for(child: &mut Running, within: Duration) -> Option<ExitStatus> {
    let started = Instant::now();
    while started.elapsed() < within {
        if let Some(status) = child.0.try_wait().unwrap() {
            return Some(status);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

fn requests(folder: &Path) -> Option<EgressRequests> {
    let read = std::fs::read(folder.join(cockpit_sandbox::REQUESTS_FILE)).ok()?;
    Some(serde_json::from_slice(&read).unwrap())
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
fn an_unknown_host_is_refused_at_once_and_asked_for_the_projects_containers() {
    let engine = engine();
    let _proxy = engine.proxy();

    let started = Instant::now();
    let (status, mut reader) = tunnel(&engine, "unknown.example.com:443");
    let elapsed = started.elapsed();
    let mut body = String::new();
    reader.read_to_string(&mut body).unwrap();

    assert!(status.starts_with("HTTP/1.1 403"), "{status}");
    assert!(body.contains("for the containers of project app"), "{body}");
    assert!(elapsed < Duration::from_secs(1), "{elapsed:?}");
    let started = Instant::now();
    let listed = loop {
        if let Some(listed) = requests(&engine.folder()) {
            break listed;
        }
        assert!(started.elapsed() < Duration::from_secs(10), "nothing asked");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        listed.subject,
        Subject::Containers {
            project: "app".to_owned()
        }
    );
    assert_eq!(listed.asked.len(), 1);
    assert_eq!(listed.asked[0].host, "unknown.example.com");
}

#[test]
fn a_destination_the_owner_allowed_tunnels() {
    let engine = engine();
    let echo = echo_service();
    engine.decide(&EgressDecisions {
        allowed: vec![AllowEntry {
            target: Target::Address(IpAddr::from([127, 0, 0, 1])),
            port: echo,
            source: AllowSource::Grant {
                grant: "1".to_owned(),
            },
        }],
        ..EgressDecisions::default()
    });
    let _proxy = engine.proxy();

    let (status, mut reader) = tunnel(&engine, &format!("127.0.0.1:{echo}"));
    reader.get_mut().write_all(b"hello\n").unwrap();
    let mut echoed = String::new();
    reader.read_line(&mut echoed).unwrap();

    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert_eq!(echoed, "echo: hello\n");
}

/// #390, option A: the engine's containers have no egress while no proxy serves them.
#[test]
fn the_proxy_ends_with_its_relay_and_the_egress_port_closes_with_it() {
    let mut engine = engine();
    let mut proxy = engine.proxy();
    drop(connect(&engine));

    let _ = engine.relay.0.kill();
    let _ = engine.relay.0.wait();

    let ended = wait_for(&mut proxy, Duration::from_secs(10));
    assert!(ended.is_some_and(|status| !status.success()), "{ended:?}");
    assert!(TcpStream::connect(("127.0.0.1", engine.port)).is_err());
}

#[test]
fn a_relay_not_run_as_the_engines_user_is_refused() {
    let engine = engine();
    let mut proxy = engine.proxy_as(engine.uid() + 1);

    let ended = wait_for(&mut proxy, Duration::from_secs(10));

    assert!(
        ended.is_some_and(|status| status.code() == Some(125)),
        "{ended:?}"
    );
    assert!(TcpStream::connect(("127.0.0.1", engine.port)).is_err());
}

#[test]
fn a_second_proxy_for_the_engine_is_refused_while_the_first_serves() {
    let engine = engine();
    let _first = engine.proxy();
    drop(connect(&engine));

    let mut second = engine.proxy();
    let ended = wait_for(&mut second, Duration::from_secs(10));

    assert!(
        ended.is_some_and(|status| status.code() == Some(125)),
        "{ended:?}"
    );
    let (status, _) = tunnel(&engine, "unknown.example.com:443");
    assert!(
        status.starts_with("HTTP/1.1 403"),
        "the first stopped serving: {status}"
    );
}

/// The daemon makes the folder; without it the proxy takes no listener it could not account
/// for.
#[test]
fn a_proxy_without_its_folder_refuses_to_start_and_takes_no_listener() {
    let engine = engine();
    std::fs::remove_dir(engine.folder()).unwrap();
    let mut proxy = engine.proxy();

    let ended = wait_for(&mut proxy, Duration::from_secs(10));

    assert!(
        ended.is_some_and(|status| status.code() == Some(125)),
        "{ended:?}"
    );
    assert!(TcpStream::connect(("127.0.0.1", engine.port)).is_err());
}
