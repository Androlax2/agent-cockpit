use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cockpit_sandbox::{
    AllowSource, AllowedService, Allowlist, AskedService, DecidedService, EgressDecisions,
    EgressRequests, Endpoint, Forward, ServiceDecisions, Subject,
};

use super::{Listener, bind_sockets};
use crate::proxy::{Network, Proxy};

/// A server on the host's loopback that answers `greeting`, then echoes what it reads.
fn echo_server(greeting: &'static [u8]) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            std::thread::spawn(move || {
                let _ = stream.write_all(greeting);
                let mut reader = stream.try_clone().unwrap();
                let _ = std::io::copy(&mut reader, &mut stream);
            });
        }
    });
    port
}

fn port_forward(service: &str, target: u16) -> Forward {
    Forward {
        service: service.to_owned(),
        listen: Endpoint::Port { port: 7979 },
        target: Endpoint::Port { port: target },
    }
}

struct Running {
    address: SocketAddr,
    proxy: Arc<Proxy<Network>>,
    requests: PathBuf,
    _folder: tempfile::TempDir,
}

/// Serves `forward` on a port of the test host's loopback, as the launcher serves the
/// listener stage 2 bound inside the sandbox.
fn serve(forward: &Forward) -> Running {
    let folder = tempfile::tempdir().unwrap();
    let requests = folder.path().join(cockpit_sandbox::REQUESTS_FILE);
    let proxy = Proxy::new(
        Allowlist::default(),
        Subject::Session {
            session: "s1".to_owned(),
        },
        Network,
        None,
    );
    proxy.ask_through(requests.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let serving = Arc::clone(&proxy);
    let served = forward.clone();
    std::thread::spawn(move || serving.serve_forward(served, Listener::Port(listener)));
    Running {
        address,
        proxy,
        requests,
        _folder: folder,
    }
}

fn allowing(forward: &Forward, source: AllowSource) -> EgressDecisions {
    EgressDecisions {
        services: ServiceDecisions {
            allowed: vec![AllowedService {
                service: forward.service.clone(),
                record: forward.record(),
                source,
                live: None,
            }],
            ..ServiceDecisions::default()
        },
        ..EgressDecisions::default()
    }
}

/// Everything `stream` sends after `then` until it closes or two seconds pass.
fn exchange(mut stream: impl Read + Write, then: &[u8]) -> String {
    let _ = stream.write_all(then);
    let mut read = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut buffer = [0_u8; 1024];
    while Instant::now() < deadline {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(count) => {
                read.extend_from_slice(&buffer[..count]);
                if read.ends_with(then) && !then.is_empty() {
                    break;
                }
            }
        }
    }
    String::from_utf8_lossy(&read).into_owned()
}

fn connect(address: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
}

fn asked_services(path: &Path) -> Vec<AskedService> {
    let text = std::fs::read(path).unwrap();
    serde_json::from_slice::<EgressRequests>(&text)
        .unwrap()
        .services
}

/// Waits until `check` holds of the report, for the counters a relay thread updates.
fn until_counted(proxy: &Proxy<Network>, check: impl Fn(&cockpit_sandbox::EgressCounters) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let report = proxy.report();
        if report.forwards.values().any(&check) {
            return;
        }
        assert!(Instant::now() < deadline, "{report:?}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn an_ungranted_service_is_closed_at_once_and_its_record_asked_of_the_owner() {
    let forward = port_forward("figcraft", echo_server(b"hello\n"));
    let running = serve(&forward);

    let started = Instant::now();
    let answer = exchange(connect(running.address), b"");

    assert_eq!(answer, "");
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(
        asked_services(&running.requests),
        vec![AskedService::of(&forward)]
    );
    until_counted(&running.proxy, |counters| {
        counters.refused == 1
            && counters
                .last_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("service figcraft is not granted"))
    });
}

#[test]
fn a_granted_service_relays_both_ways_and_counts_the_connection_and_bytes() {
    let forward = port_forward("figcraft", echo_server(b"hello\n"));
    let running = serve(&forward);
    running
        .proxy
        .apply_decisions(allowing(&forward, AllowSource::Request { request: 4 }));

    let answer = exchange(connect(running.address), b"ping");

    assert_eq!(answer, "hello\nping");
    until_counted(&running.proxy, |counters| {
        counters.allowed == 1
            && counters.bytes_out == 4
            && counters.bytes_in == 10
            && counters.source.as_deref() == Some("network request 4, allowed for this session")
    });
}

#[test]
fn a_grant_for_another_target_does_not_allow_the_service() {
    let forward = port_forward("figcraft", echo_server(b"hello\n"));
    let edited = port_forward("figcraft", forward.listen_port().unwrap());
    let running = serve(&forward);
    running.proxy.apply_decisions(allowing(
        &edited,
        AllowSource::Grant {
            grant: "3".to_owned(),
        },
    ));

    let answer = exchange(connect(running.address), b"ping");

    assert_eq!(answer, "");
    assert_eq!(
        asked_services(&running.requests),
        vec![AskedService::of(&forward)]
    );
}

#[test]
fn a_denied_service_is_closed_naming_the_denial_without_asking_again() {
    let forward = port_forward("figcraft", echo_server(b"hello\n"));
    let running = serve(&forward);
    running.proxy.apply_decisions(EgressDecisions {
        services: ServiceDecisions {
            denied: vec![DecidedService {
                service: forward.service.clone(),
                record: forward.record(),
                request: 9,
            }],
            ..ServiceDecisions::default()
        },
        ..EgressDecisions::default()
    });

    assert_eq!(exchange(connect(running.address), b"ping"), "");

    assert!(!running.requests.exists());
    until_counted(&running.proxy, |counters| {
        counters.last_reason.as_deref() == Some("the owner denied service request 9")
    });
}

#[test]
fn revoking_the_grant_closes_an_open_relay_and_refuses_the_next_connection() {
    let forward = port_forward("figcraft", echo_server(b"hello\n"));
    let running = serve(&forward);
    running.proxy.apply_decisions(allowing(
        &forward,
        AllowSource::Grant {
            grant: "3".to_owned(),
        },
    ));
    let mut open = connect(running.address);
    let mut greeting = [0_u8; 6];
    open.read_exact(&mut greeting).unwrap();

    running.proxy.apply_decisions(EgressDecisions::default());

    let mut rest = Vec::new();
    let closed = open.read_to_end(&mut rest);
    assert!(closed.is_ok_and(|count| count == 0), "{rest:?}");
    assert_eq!(exchange(connect(running.address), b"ping"), "");
}

#[test]
fn an_unreachable_target_is_counted_apart_from_refusals() {
    let unused = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let forward = port_forward("figcraft", unused);
    let running = serve(&forward);
    running
        .proxy
        .apply_decisions(allowing(&forward, AllowSource::Request { request: 1 }));

    assert_eq!(exchange(connect(running.address), b"ping"), "");

    until_counted(&running.proxy, |counters| {
        counters.unreachable == 1 && counters.refused == 0
    });
}

#[test]
fn a_socket_target_is_relayed_from_a_socket_listen_the_launcher_created() {
    let folder = tempfile::tempdir().unwrap();
    let target = folder.path().join("service.sock");
    let server = UnixListener::bind(&target).unwrap();
    std::thread::spawn(move || {
        for stream in server.incoming() {
            let mut stream = stream.unwrap();
            let _ = stream.write_all(b"from the socket\n");
        }
    });
    let forward = Forward {
        service: "engine".to_owned(),
        listen: Endpoint::Socket {
            path: PathBuf::from("/run/torlek/engine.sock"),
        },
        target: Endpoint::Socket { path: target },
    };
    let mount_dir = folder.path().join("mounts");
    let super::BoundSockets {
        folders,
        mut listeners,
    } = bind_sockets(&mount_dir, std::slice::from_ref(&forward)).unwrap();
    assert_eq!(folders.len(), 1);
    assert_eq!(folders[0].destination, Path::new("/run/torlek"));
    let (index, listener) = listeners.pop().unwrap();
    assert_eq!(index, 0);
    let proxy = Proxy::new(
        Allowlist::default(),
        Subject::Session {
            session: "s1".to_owned(),
        },
        Network,
        None,
    );
    proxy.apply_decisions(allowing(&forward, AllowSource::Request { request: 2 }));
    let serving = Arc::clone(&proxy);
    let served = forward.clone();
    std::thread::spawn(move || serving.serve_forward(served, Listener::Socket(listener)));

    let client = UnixStream::connect(folders[0].source.join("engine.sock")).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();

    assert_eq!(exchange(client, b""), "from the socket\n");
}

#[test]
fn socket_listens_in_one_folder_share_one_bound_folder() {
    let folder = tempfile::tempdir().unwrap();
    let socket = |service: &str| Forward {
        service: service.to_owned(),
        listen: Endpoint::Socket {
            path: PathBuf::from(format!("/run/torlek/{service}.sock")),
        },
        target: Endpoint::Port { port: 1 },
    };
    let forwards = [socket("one"), port_forward("port", 2), socket("two")];

    let super::BoundSockets { folders, listeners } =
        bind_sockets(folder.path(), &forwards).unwrap();

    assert_eq!(folders.len(), 1);
    let indexes: Vec<usize> = listeners.iter().map(|(index, _)| *index).collect();
    assert_eq!(indexes, [0, 2]);
}

#[test]
fn a_socket_target_whose_backlog_is_full_fails_within_the_timeout() {
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketType, bind, listen, socket};

    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("full.sock");
    let server = socket(AddressFamily::UNIX, SocketType::STREAM, None).unwrap();
    bind(&server, &SocketAddrUnix::new(path.as_path()).unwrap()).unwrap();
    // A backlog of 0 holds one connection nobody accepts; the next one waits for room.
    listen(&server, 0).unwrap();
    let _waiting = UnixStream::connect(&path).unwrap();

    let started = Instant::now();
    let refused = super::connect_socket_within(&path, Duration::from_millis(200));

    assert!(refused.is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
}
