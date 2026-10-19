//! claude-remote, the phone page: driven over HTTP as tailscale serve would, on a server started
//! for each test.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Stdio};

struct Remote {
    sandbox: Sandbox,
    server: Child,
    port: u16,
}

impl Remote {
    fn start() -> Remote {
        let sandbox = Sandbox::new();
        sandbox.shim("tailscale", r#"echo '{"Self": {"DNSName": "box.tail1234.ts.net."}}'"#);
        sandbox.shim("qrencode", "");
        // capture-pane shows the file pane.txt, as a session's pane would.
        sandbox.shim("tmux", r#"[[ $1 == capture-pane ]] && cat "$XDG_RUNTIME_DIR/pane.txt" 2>/dev/null; exit 0"#);
        let mut server = sandbox.command(bin().join("claude-remote"), &[("CLAUDE_REMOTE_ADDRESS", "127.0.0.1:0")]).arg("serve")
            .stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(server.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let port = line.trim().rsplit(':').next().unwrap().parse().unwrap_or_else(|_| panic!("no port in {line:?}"));
        Remote { sandbox, server, port }
    }

    /// The status, the headers and the body of one request.
    fn request(&self, method: &str, path: &str, cookie: Option<&str>, body: Option<Value>, from_page: bool) -> (u16, String, String) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let body = body.map(|body| body.to_string()).unwrap_or_default();
        let mut head = format!("{method} {path} HTTP/1.1\r\nHost: box.tail1234.ts.net\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
        if let Some(cookie) = cookie {
            head.push_str(&format!("Cookie: claude_remote={cookie}\r\n"));
        }
        if from_page {
            head.push_str("X-Claude-Remote: 1\r\nContent-Type: application/json\r\n");
        }
        stream.write_all(format!("{head}\r\n{body}").as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        let (headers, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        (status, headers.to_string(), body.to_string())
    }

    /// A one-time pairing code, from `claude-remote pair`'s link.
    fn pairing_code(&self) -> String {
        let output = self.sandbox.run(bin().join("claude-remote"), &["pair"], &[]);
        let shown = stdout(&output);
        shown.split("/pair/").nth(1).and_then(|rest| rest.split_whitespace().next()).unwrap_or_else(|| panic!("no link in {shown}")).to_string()
    }

    /// A paired phone's secret.
    fn paired(&self) -> String {
        let (status, headers, _) = self.request("GET", &format!("/pair/{}", self.pairing_code()), None, None, false);
        assert_eq!(status, 303, "{headers}");
        headers.lines().find_map(|line| line.strip_prefix("Set-Cookie: claude_remote=")).unwrap().split(';').next().unwrap().to_string()
    }

    /// An approval asking now, as claude-run-approve leaves it.
    fn approval_asking(&self, id: &str, command: &str) {
        let runs = self.sandbox.runtime.join("claude-runs");
        fs::create_dir_all(&runs).unwrap();
        fs::write(runs.join(format!("{id}.asking")), json!({"id": id, "project": "proj-a", "cwd": "/p", "command": command, "why": "x"}).to_string()).unwrap();
    }

    /// Session A in the snapshot, its pane %4 showing `pane`.
    fn session_showing(&self, pane: &str) {
        fs::write(self.sandbox.runtime.join("claude-sessions.json"),
                  json!({"sessions": [{"session": SESSION_A, "state": "needs-you", "project": "proj-a", "tmux_pane": "%4"}]}).to_string()).unwrap();
        fs::write(self.sandbox.runtime.join("pane.txt"), pane).unwrap();
    }

    fn keys_pressed(&self) -> Vec<String> {
        self.sandbox.calls("tmux").into_iter().filter(|call| call.starts_with("tmux send-keys")).collect()
    }
}

const DIALOG: &str = " Bash command\n   npm test\n Do you want to proceed?\n ❯ 1. Yes\n   2. No\n";
const IDLE: &str = "done\n❯ \n";

impl Drop for Remote {
    fn drop(&mut self) {
        let _ = self.server.kill();
    }
}

#[test]
fn an_unpaired_phone_gets_nothing() {
    let remote = Remote::start();
    for path in ["/", "/api/state", "/page.js"] {
        assert_eq!(remote.request("GET", path, None, None, false).0, 401, "{path}");
        assert_eq!(remote.request("GET", path, Some(&"0".repeat(64)), None, false).0, 401, "{path} with a made-up secret");
    }
}

#[test]
fn a_pairing_link_works_once() {
    let remote = Remote::start();
    let code = remote.pairing_code();
    let (status, headers, _) = remote.request("GET", &format!("/pair/{code}"), None, None, false);
    assert_eq!(status, 303);
    assert!(headers.contains("Secure; HttpOnly; SameSite=Strict"), "{headers}");
    assert_eq!(remote.request("GET", &format!("/pair/{code}"), None, None, false).0, 403, "used twice");
}

#[test]
fn a_paired_phone_sees_the_sessions() {
    let remote = Remote::start();
    let secret = remote.paired();
    fs::write(remote.sandbox.runtime.join("claude-sessions.json"),
              json!({"sessions": [{"session": SESSION_A, "state": "needs-you", "project": "proj-a", "task": "Deploy", "request": "Bash: make deploy"}]}).to_string()).unwrap();
    let (status, _, body) = remote.request("GET", "/api/state", Some(&secret), None, false);
    assert_eq!(status, 200);
    let state: Value = serde_json::from_str(&body).unwrap();
    assert_eq!((state["sessions"][0]["project"].as_str(), state["sessions"][0]["request"].as_str()), (Some("proj-a"), Some("Bash: make deploy")));
}

#[test]
fn a_post_not_from_the_page_is_refused() {
    // Another site can make the phone's browser post, but not add this header.
    let remote = Remote::start();
    let secret = remote.paired();
    remote.approval_asking("0123456789ab", "make test");
    let (status, _, _) = remote.request("POST", "/api/decide/0123456789ab", Some(&secret), Some(json!({"run": true, "command": "make test"})), false);
    assert_eq!(status, 403);
    assert!(!remote.sandbox.runtime.join("claude-runs/0123456789ab.remote").exists());
}

#[test]
fn run_from_the_phone_answers_the_approval_that_asks() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.approval_asking("0123456789ab", "make test");
    let (status, _, body) = remote.request("POST", "/api/decide/0123456789ab", Some(&secret), Some(json!({"run": true, "command": "make test"})), true);
    assert_eq!(status, 200, "{body}");
    assert_eq!(fs::read_to_string(remote.sandbox.runtime.join("claude-runs/0123456789ab.remote")).unwrap(), "y");
}

#[test]
fn a_command_other_than_the_one_shown_is_not_run() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.approval_asking("0123456789ab", "make test");
    let (status, _, body) = remote.request("POST", "/api/decide/0123456789ab", Some(&secret), Some(json!({"run": true, "command": "make tset"})), true);
    assert_eq!(status, 409, "{body}");
    assert!(!remote.sandbox.runtime.join("claude-runs/0123456789ab.remote").exists());
}

#[test]
fn an_approval_that_asks_nothing_takes_no_answer() {
    let remote = Remote::start();
    let secret = remote.paired();
    let (status, _, _) = remote.request("POST", "/api/decide/0123456789ab", Some(&secret), Some(json!({"run": true, "command": "make test"})), true);
    assert_eq!(status, 409);
}

#[test]
fn unpair_forgets_every_phone() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.sandbox.run(bin().join("claude-remote"), &["unpair"], &[]);
    assert_eq!(remote.request("GET", "/api/state", Some(&secret), None, false).0, 401);
}

#[test]
fn the_server_listens_on_a_loopback_address_only() {
    // tailscale serve publishes it on the tailnet; anywhere else the home network would reach it.
    let sandbox = Sandbox::new();
    let output = sandbox.run(bin().join("claude-remote"), &["serve"], &[("CLAUDE_REMOTE_ADDRESS", "0.0.0.0:0")]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("loopback"), "{}", stderr(&output));
}

#[test]
fn every_phone_action_is_audited() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.approval_asking("0123456789ab", "make test");
    remote.request("POST", "/api/decide/0123456789ab", Some(&secret), Some(json!({"run": false, "command": "make test"})), true);
    let folder = remote.sandbox.state.join("claude-audit");
    let events: Vec<String> = fs::read_dir(&folder).unwrap().flatten().filter(|entry| entry.file_name().to_string_lossy().starts_with("audit-"))
        .flat_map(|entry| fs::read_to_string(entry.path()).unwrap().lines().map(|line| serde_json::from_str::<Value>(line).unwrap()["event"].as_str().unwrap().to_string()).collect::<Vec<_>>())
        .collect();
    assert!(events.contains(&"remote.paired".to_string()) && events.contains(&"remote.decline".to_string()), "{events:?}");
}

#[test]
fn yes_and_no_from_the_phone_press_the_dialog_keys() {
    for (is_yes, key) in [(true, "Enter"), (false, "Escape")] {
        let remote = Remote::start();
        let secret = remote.paired();
        remote.session_showing(DIALOG);
        let (status, _, body) = remote.request("POST", &format!("/api/answer/{SESSION_A}"), Some(&secret), Some(json!({"yes": is_yes})), true);
        assert_eq!(status, 200, "{body}");
        assert_eq!(remote.keys_pressed(), [format!("tmux send-keys -t %4 {key}")]);
    }
}

#[test]
fn nothing_is_pressed_when_the_dialog_closed() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.session_showing(IDLE);
    let (status, _, body) = remote.request("POST", &format!("/api/answer/{SESSION_A}"), Some(&secret), Some(json!({"yes": true})), true);
    assert_eq!(status, 409, "{body}");
    assert!(remote.keys_pressed().is_empty());
}

#[test]
fn yes_is_refused_when_the_cursor_left_yes() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.session_showing(&DIALOG.replace("❯ 1. Yes\n   2. No", "  1. Yes\n ❯ 2. No"));
    let (status, _, body) = remote.request("POST", &format!("/api/answer/{SESSION_A}"), Some(&secret), Some(json!({"yes": true})), true);
    assert_eq!(status, 409, "{body}");
    assert!(remote.keys_pressed().is_empty());
}

#[test]
fn health_answers_without_a_phone_and_tells_nothing_else() {
    let remote = Remote::start();
    let (status, _, body) = remote.request("GET", "/health", None, None, false);
    assert_eq!((status, body.as_str()), (200, "ok"));
}

#[test]
fn a_slow_request_does_not_stall_the_others() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.sandbox.shim("tmux", "sleep 3");
    fs::write(remote.sandbox.runtime.join("claude-sessions.json"),
              json!({"sessions": [{"session": SESSION_A, "state": "waiting", "project": "proj-a", "tmux_pane": "%4"}]}).to_string()).unwrap();
    let port = remote.port;
    let slow_secret = secret.clone();
    let slow = std::thread::spawn(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.write_all(format!("GET /api/screen/{SESSION_A} HTTP/1.1\r\nCookie: claude_remote={slow_secret}\r\nConnection: close\r\n\r\n").as_bytes()).unwrap();
        let mut response = String::new();
        let _ = stream.read_to_string(&mut response);
    });
    std::thread::sleep(std::time::Duration::from_millis(200));
    let started = std::time::Instant::now();
    assert_eq!(remote.request("GET", "/api/state", Some(&secret), None, false).0, 200);
    assert!(started.elapsed() < std::time::Duration::from_secs(1), "waited {:?} behind the slow request", started.elapsed());
    slow.join().unwrap();
}
