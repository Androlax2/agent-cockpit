//! `torlek-sandbox run-egress` and `hand-over` (#1387), the real binary: a program in a
//! network namespace with nothing but loopback reaches the hosts its card names through the
//! proxy, and the proxy names each other destination it refused. bubblewrap gives the
//! program that namespace here, as the run's systemd service does in the daemon. Needs
//! `bwrap` on `PATH`, as CI installs it.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Output, Stdio};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_torlek-sandbox");

/// A host on the test's network that answers one HTTP request with `fixture`.
fn fixture_host(address: &str) -> String {
    let listener = TcpListener::bind(format!("{address}:0")).unwrap();
    let host = listener.local_addr().unwrap().to_string();
    std::thread::spawn(move || {
        for client in listener.incoming() {
            let mut client = client.unwrap();
            let mut head = [0_u8; 1024];
            let _ = client.read(&mut head);
            let _ = client.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 7\r\n\r\nfixture");
        }
    });
    host
}

struct Egress {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    child: Child,
    stdout: BufReader<ChildStdout>,
}

impl Egress {
    /// The proxy of run 7, whose card names `hosts`, once it says it is ready.
    fn serving(hosts: &[&str]) -> Egress {
        let dir = tempfile::tempdir().unwrap();
        let mut child = Command::new(LAUNCHER)
            .arg("run-egress")
            .arg(dir.path())
            .args(["s1", "7"])
            .args(hosts)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        stdout.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n");
        Egress {
            socket: dir.path().join("proxy-handoff").join("handoff.sock"),
            _dir: dir,
            child,
            stdout,
        }
    }

    /// `curl` to `host` from a network namespace of its own, which hands the proxy its
    /// listener first.
    fn curl(&self, host: &str) -> Output {
        Command::new("bwrap")
            .args(["--dev-bind", "/", "/", "--unshare-net", "--"])
            .arg(LAUNCHER)
            .arg("hand-over")
            .arg(&self.socket)
            .args(["curl", "-sS", "--max-time", "10", "--proxytunnel"])
            .arg(format!("http://{host}/"))
            .env("http_proxy", "http://127.0.0.1:3128")
            .output()
            .expect("bubblewrap (`bwrap`) on PATH")
    }

    /// What the proxy says once the run is over: the destinations it refused.
    fn over(mut self) -> String {
        drop(self.child.stdin.take());
        let mut rest = String::new();
        self.stdout.read_to_string(&mut rest).unwrap();
        assert!(self.child.wait().unwrap().success());
        rest
    }
}

#[test]
fn a_host_the_card_names_is_reached_from_a_namespace_without_a_network() {
    let host = fixture_host("127.0.0.2");
    let egress = Egress::serving(&[&host]);

    let curled = egress.curl(&host);

    assert!(curled.status.success(), "{curled:?}");
    assert_eq!(String::from_utf8_lossy(&curled.stdout), "fixture");
    assert_eq!(egress.over(), "");
}

#[test]
fn a_host_the_card_does_not_name_is_refused_and_named_once_the_run_is_over() {
    let named = fixture_host("127.0.0.2");
    let unnamed = fixture_host("127.0.0.3");
    let egress = Egress::serving(&[&named]);

    let curled = egress.curl(&unnamed);

    assert!(!curled.status.success(), "{curled:?}");
    assert_eq!(egress.over(), format!("refused {unnamed}\n"));
}

#[test]
fn a_run_over_before_it_handed_its_listener_over_ends_the_proxy() {
    let egress = Egress::serving(&["example.com"]);

    assert_eq!(egress.over(), "");
}
