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
    fn restart(&mut self) {
        self.server.kill().unwrap();
        self.server.wait().unwrap();
        let mut server = self.sandbox.command(env!("CARGO_BIN_EXE_cockpit-remote"), &[("COCKPIT_REMOTE_ADDRESS", "127.0.0.1:0")])
            .arg("serve").stdout(Stdio::piped()).spawn().unwrap();
        let mut line = String::new();
        BufReader::new(server.stdout.take().unwrap()).read_line(&mut line).unwrap();
        self.port = line.trim().rsplit(':').next().unwrap().parse().unwrap();
        self.server = server;
    }

    fn start() -> Remote {
        let sandbox = Sandbox::new();
        sandbox.shim("tailscale", r#"echo '{"Self": {"DNSName": "box.tail1234.ts.net."}}'"#);
        sandbox.shim("qrencode", "");
        // capture-pane shows the file pane.txt, as a session's pane would.
        sandbox.shim("tmux", r#"[[ $1 == capture-pane ]] && cat "$XDG_RUNTIME_DIR/pane.txt" 2>/dev/null; exit 0"#);
        let mut server = sandbox.command(env!("CARGO_BIN_EXE_cockpit-remote"), &[("CLAUDE_REMOTE_ADDRESS", "127.0.0.1:0")]).arg("serve")
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
        let mut head = format!("{method} {path} HTTP/1.0\r\nHost: box.tail1234.ts.net\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
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
        self.pairing_code_for(&[])
    }

    fn pairing_code_for(&self, name: &[&str]) -> String {
        let output = self.sandbox.run(bin().join("claude-remote"), &[&["pair"], name].concat(), &[]);
        let shown = stdout(&output);
        shown.split("/pair/").nth(1).and_then(|rest| rest.split_whitespace().next()).unwrap_or_else(|| panic!("no link in {shown}")).to_string()
    }

    /// A paired phone's secret.
    fn paired(&self) -> String {
        self.paired_as(&[])
    }

    fn paired_as(&self, name: &[&str]) -> String {
        let (status, headers, _) = self.request("GET", &format!("/pair/{}", self.pairing_code_for(name)), None, None, false);
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
        self.session_in("needs-you", pane);
    }

    fn session_in(&self, state: &str, pane: &str) {
        fs::write(self.sandbox.runtime.join("cockpit.json"),
                  json!({"sessions": [{"session": SESSION_A, "state": state, "project": "proj-a", "tmux_pane": "%4"}]}).to_string()).unwrap();
        fs::write(self.sandbox.runtime.join("pane.txt"), pane).unwrap();
    }

    fn remote(&self, arguments: &[&str]) -> String {
        stdout(&self.sandbox.run(bin().join("claude-remote"), arguments, &[]))
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
fn an_unpaired_phone_gets_no_data() {
    let remote = Remote::start();
    for path in ["/api/state", "/api/push/key", &format!("/api/screen/{SESSION_A}")] {
        assert_eq!(remote.request("GET", path, None, None, false).0, 401, "{path}");
        assert_eq!(remote.request("GET", path, Some(&"0".repeat(64)), None, false).0, 401, "{path} with a made-up secret");
    }
    assert_eq!(remote.request("POST", "/api/pairing-code", Some(&"0".repeat(64)), Some(json!({})), true).0, 401);
}

#[test]
fn an_unpaired_phone_loads_the_page_itself_to_paste_a_code() {
    // The Home Screen app on iOS keeps cookies of its own: it starts unpaired and needs the page.
    let remote = Remote::start();
    for path in ["/", "/page.js", "/sw.js", "/manifest.webmanifest"] {
        assert_eq!(remote.request("GET", path, None, None, false).0, 200, "{path}");
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
    fs::write(remote.sandbox.runtime.join("cockpit.json"),
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
fn an_ended_approval_is_not_listed_even_when_its_asking_file_remains() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.approval_asking("0123456789ab", "make test");
    remote.sandbox.shim("tmux", "exit 1");
    let state = remote.get_json(&secret, "/api/state");
    assert_eq!(state["approvals"], json!([]));
    assert!(remote.sandbox.runtime.join("claude-runs/0123456789ab.asking").exists());
}

#[test]
fn an_approval_that_ends_after_loading_disappears_after_a_refused_action() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.approval_asking("0123456789ab", "make test");
    assert_eq!(remote.get_json(&secret, "/api/state")["approvals"].as_array().unwrap().len(), 1);
    remote.sandbox.shim("tmux", "exit 1");
    let (status, answer) = remote.post_json(&secret, "/api/decide/0123456789ab", json!({"run": true, "command": "make test"}));
    assert_eq!(status, 409, "{answer}");
    assert_eq!(answer["message"], "this approval ended");
    assert_eq!(remote.get_json(&secret, "/api/state")["approvals"], json!([]));
    assert!(!remote.sandbox.runtime.join("claude-runs/0123456789ab.remote").exists());
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
    fs::write(remote.sandbox.runtime.join("cockpit.json"),
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

#[test]
fn phones_lists_each_phone_by_its_name_without_its_secret() {
    let remote = Remote::start();
    let secret = remote.paired_as(&["iPhone"]);
    remote.paired();
    let listed = remote.remote(&["phones"]);
    assert!(listed.contains("iPhone") && listed.contains("Phone 2"), "{listed}");
    assert!(!listed.contains(&secret), "{listed}");
}

#[test]
fn unpair_with_an_id_forgets_that_phone_only() {
    let remote = Remote::start();
    let lost = remote.paired_as(&["lost"]);
    let kept = remote.paired_as(&["kept"]);
    let listed = remote.remote(&["phones"]);
    let lost_id = listed.lines().find(|line| line.contains("lost")).unwrap().split_whitespace().next().unwrap().to_string();
    assert!(remote.remote(&["unpair", &lost_id]).contains("lost forgotten"));
    assert_eq!(remote.request("GET", "/api/state", Some(&lost), None, false).0, 401);
    assert_eq!(remote.request("GET", "/api/state", Some(&kept), None, false).0, 200);
}

#[test]
fn unpair_with_an_unknown_id_fails_loudly() {
    let remote = Remote::start();
    let output = remote.sandbox.run(bin().join("claude-remote"), &["unpair", "deadbeef"], &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no paired phone deadbeef"), "{}", stderr(&output));
}

#[test]
fn interrupt_presses_escape_in_a_session_that_works_only() {
    for (state, status, pressed) in [("working", 200, vec!["tmux send-keys -t %4 Escape".to_string()]), ("waiting", 409, vec![])] {
        let remote = Remote::start();
        let secret = remote.paired();
        remote.session_in(state, IDLE);
        let (answered, _, body) = remote.request("POST", &format!("/api/interrupt/{SESSION_A}"), Some(&secret), Some(json!({})), true);
        assert_eq!(answered, status, "{state}: {body}");
        assert_eq!(remote.keys_pressed(), pressed, "{state}");
    }
}

#[test]
fn interrupt_presses_an_opencode_sessions_own_keys() {
    // Escape twice: the first only says "esc again to interrupt".
    let remote = Remote::start();
    let secret = remote.paired();
    fs::write(remote.sandbox.runtime.join("cockpit.json"),
              json!({"sessions": [{"session": OPENCODE_SESSION, "state": "working", "project": "proj-a", "tmux_pane": "%4", "agent": "opencode"}]}).to_string()).unwrap();
    let (status, _, body) = remote.request("POST", &format!("/api/interrupt/{OPENCODE_SESSION}"), Some(&secret), Some(json!({})), true);
    assert_eq!(status, 200, "{body}");
    assert_eq!(remote.keys_pressed(), ["tmux send-keys -t %4 Escape Escape"]);
}

#[test]
fn each_card_carries_what_its_agent_can_do_and_the_agents_are_listed() {
    let remote = Remote::start();
    let secret = remote.paired();
    fs::write(remote.sandbox.runtime.join("cockpit.json"), json!({"sessions": [
        {"session": SESSION_A, "state": "waiting", "project": "proj-a", "tmux_pane": "%4"},
        {"session": OPENCODE_SESSION, "state": "waiting", "project": "proj-b", "tmux_pane": "%5", "agent": "opencode"}]}).to_string()).unwrap();
    let cards = remote.get_json(&secret, "/api/state");
    let features = |id: &str| cards["sessions"].as_array().unwrap().iter().find(|card| card["session"] == id).unwrap()["features"].clone();
    assert!(features(SESSION_A).as_array().unwrap().contains(&json!("dialogs")), "{cards}");
    assert_eq!(features(OPENCODE_SESSION), json!(["interrupt", "variants", "sleep"]));
    let agents = remote.get_json(&secret, "/api/agents");
    assert_eq!(agents, json!([{"name": "claude", "label": "Claude Code"}, {"name": "opencode", "label": "opencode"}, {"name": "codex", "label": "Codex CLI"}]));
}

#[test]
fn remote_control_from_the_phone_goes_through_cockpit() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.sandbox.shim("cockpit", "exit 0");
    remote.session_in("waiting", IDLE);
    let (status, _, body) = remote.request("POST", &format!("/api/remote-control/{SESSION_A}"), Some(&secret), Some(json!({})), true);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Remote Control on"), "{body}");
    assert_eq!(remote.sandbox.calls("cockpit"), [format!("cockpit remote {SESSION_A}")]);
}

#[test]
fn the_screen_keeps_its_colours_and_drops_every_other_escape() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.session_in("waiting", "\x1b[1;32mdone\x1b[0m\x1b]0;title\x07\x1b[2J\x1b[?25l\rnext\x07\n");
    let (status, _, body) = remote.request("GET", &format!("/api/screen/{SESSION_A}"), Some(&secret), None, false);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, "\x1b[1;32mdone\x1b[0mnext\n");
}

impl Remote {
    fn claude_history(&self, records: &[Value]) -> std::path::PathBuf {
        let folder = self.sandbox.home.join(".claude/projects/proj-a");
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join(format!("{SESSION_A}.jsonl"));
        fs::write(&path, records.iter().map(|record| format!("{record}\n")).collect::<String>()).unwrap();
        path
    }
}

const RUNNING: &str = "0123456789ab";

impl Remote {
    /// A command approved and running, waiting on `prompt` after printing `output`, as approval::run
    /// leaves it.
    fn command_asking(&self, prompt: &str, output: &str) {
        let runs = self.sandbox.runtime.join("claude-runs");
        fs::create_dir_all(&runs).unwrap();
        fs::write(runs.join(format!("{RUNNING}.input")), json!({"id": RUNNING, "command": "cockpit apply /clone", "prompt": prompt}).to_string()).unwrap();
        fs::write(runs.join(format!("{RUNNING}.log")), format!("Script started on 2026-10-22\n{output}{prompt} ")).unwrap();
    }
}

#[test]
fn a_command_waiting_on_a_yes_no_question_is_listed() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.command_asking("Apply these 1 commit(s)? [y/N]", "1 commit(s)\n");
    assert_eq!(remote.get_json(&secret, "/api/state")["inputs"],
               json!([{"id": RUNNING, "command": "cockpit apply /clone", "prompt": "Apply these 1 commit(s)? [y/N]"}]));
}

#[test]
fn a_question_for_a_secret_is_left_to_the_computer() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.command_asking("[sudo] password for theo:", "");
    assert_eq!(remote.get_json(&secret, "/api/state")["inputs"], json!([]));
    let (status, answer) = remote.post_json(&secret, &format!("/api/input/{RUNNING}"), json!({"yes": true, "prompt": "[sudo] password for theo:"}));
    assert_eq!(status, 409, "{answer}");
    assert!(remote.keys_pressed().is_empty(), "{:?}", remote.keys_pressed());
}

#[test]
fn the_phone_reads_what_the_waiting_command_printed_without_escapes_or_secrets() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.command_asking("Apply? [y/N]", "\x1b[1mSubject: fix\x1b[0m\nGITHUB_TOKEN=ghp_0123456789abcdefghijABCDEFGHIJ012345\n");
    let (status, _, body) = remote.request("GET", &format!("/api/input/{RUNNING}"), Some(&secret), None, false);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("Subject: fix\n") && body.trim_end().ends_with("Apply? [y/N]"), "{body:?}");
    assert!(!body.contains('\x1b') && !body.contains("ghp_0123"), "{body:?}");
}

#[test]
fn an_answer_is_typed_in_the_approval_window_as_the_question_asks() {
    for (is_yes, prompt, typed) in [(true, "Apply? [y/N]", "y"), (false, "Apply? [y/N]", "n"), (true, "Continue connecting (yes/no)?", "yes")] {
        let remote = Remote::start();
        let secret = remote.paired();
        remote.command_asking(prompt, "");
        let (status, answer) = remote.post_json(&secret, &format!("/api/input/{RUNNING}"), json!({"yes": is_yes, "prompt": prompt}));
        assert_eq!(status, 200, "{answer}");
        assert_eq!(remote.keys_pressed(), [format!("tmux send-keys -t =claude-run-{RUNNING}: -l {typed}"), format!("tmux send-keys -t =claude-run-{RUNNING}: Enter")]);
        assert!(!remote.sandbox.runtime.join(format!("claude-runs/{RUNNING}.input")).exists(), "one answer per question");
    }
}

#[test]
fn an_answer_to_a_question_that_changed_is_refused() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.command_asking("Delete 3 branches? [y/N]", "");
    let (status, answer) = remote.post_json(&secret, &format!("/api/input/{RUNNING}"), json!({"yes": true, "prompt": "Apply? [y/N]"}));
    assert_eq!(status, 409, "{answer}");
    assert!(remote.keys_pressed().is_empty(), "{:?}", remote.keys_pressed());
}

#[test]
fn an_answer_to_a_command_that_ended_is_refused() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.command_asking("Apply? [y/N]", "");
    remote.sandbox.shim("tmux", "exit 1");
    let (status, answer) = remote.post_json(&secret, &format!("/api/input/{RUNNING}"), json!({"yes": true, "prompt": "Apply? [y/N]"}));
    assert_eq!(status, 409, "{answer}");
    assert_eq!(answer["message"], "this command ended");
    assert!(remote.keys_pressed().is_empty(), "{:?}", remote.keys_pressed());
}

fn card_title(remote: &Remote, secret: &str, row: Value) -> Value {
    fs::write(remote.sandbox.runtime.join("cockpit.json"), json!({"sessions": [row]}).to_string()).unwrap();
    remote.get_json(secret, "/api/state")["sessions"][0]["title"].clone()
}

#[test]
fn a_card_is_titled_by_its_session_label_first() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.claude_history(&[json!({"type": "ai-title", "aiTitle": "Agent title"})]);
    let row = json!({"session": SESSION_A, "state": "waiting", "project": "proj-a", "label": "Fix phone session titles"});
    assert_eq!(card_title(&remote, &secret, row), json!("Fix phone session titles"));
}

#[test]
fn a_card_without_a_label_is_titled_by_its_agent() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.claude_history(&[json!({"type": "ai-title", "aiTitle": "Remote access to desktop from phone"})]);
    let row = json!({"session": SESSION_A, "state": "waiting", "project": "proj-a"});
    assert_eq!(card_title(&remote, &secret, row), json!("Remote access to desktop from phone"));
}

#[test]
fn a_card_with_neither_has_an_empty_title() {
    let remote = Remote::start();
    let secret = remote.paired();
    let row = json!({"session": SESSION_A, "state": "waiting", "project": "proj-a"});
    assert_eq!(card_title(&remote, &secret, row), json!(""));
}

#[test]
fn the_phone_reads_only_visible_conversation_text_in_order() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.session_in("waiting", IDLE);
    remote.claude_history(&[
        json!({"uuid": "u1", "type": "user", "message": {"content": "Fix login"}}),
        json!({"uuid": "hidden", "type": "system", "message": {"content": "secret system prompt"}}),
        json!({"uuid": "a1", "type": "assistant", "message": {"content": [
            {"type": "thinking", "thinking": "private reasoning"}, {"type": "tool_use", "input": "secret"},
            {"type": "text", "text": "<script>alert(1)</script>"}]}}),
        json!({"uuid": "tool", "type": "user", "message": {"content": [{"type": "tool_result", "content": "secret output"}]}}),
    ]);
    assert_eq!(remote.get_json(&secret, &format!("/api/conversation/{SESSION_A}")), json!({
        "messages": [{"id": "u1", "role": "user", "text": "Fix login"},
            {"id": "a1", "role": "assistant", "text": "<script>alert(1)</script>"}], "truncated": false}));
}

#[test]
fn the_phone_reads_native_codex_history_through_a_verified_thread_mapping() {
    let remote = Remote::start();
    let secret = remote.paired();
    let native = "aaaaaaaa-0000-4000-8000-000000000000";
    let session = cockpit::codex::cockpit_id(native).unwrap();
    fs::write(remote.sandbox.runtime.join("cockpit.json"), json!({"sessions":[{
        "session":session, "agent":"codex", "native_thread":native, "state":"waiting"
    }]}).to_string()).unwrap();
    fs::write(remote.sandbox.home.join("snapshot.json"), json!({"thread":native, "model":"gpt-6.1-sol", "effort":"high",
        "messages":[{"id":"request", "role":"user", "text":"Continue this work"}], "latest_user":"Continue this work", "truncated":false
    }).to_string()).unwrap();
    remote.sandbox.shim("python3", r#"cat "$HOME/snapshot.json""#);
    assert_eq!(remote.get_json(&secret, &format!("/api/conversation/{session}")), json!({
        "messages":[{"id":"request", "role":"user", "text":"Continue this work"}], "truncated":false
    }));
    assert_eq!(remote.sandbox.calls("python3"), [format!("python3 {} {native}", remote.sandbox.home.join(".config/waybar/scripts/codex-session.py").display())]);
}

#[test]
fn a_native_thread_mapping_mismatch_is_refused_before_reading_history() {
    let remote = Remote::start();
    let secret = remote.paired();
    let session = cockpit::codex::cockpit_id("aaaaaaaa-0000-4000-8000-000000000000").unwrap();
    fs::write(remote.sandbox.runtime.join("cockpit.json"), json!({"sessions":[{
        "session":session, "agent":"codex", "native_thread":"bbbbbbbb-0000-4000-8000-000000000000", "state":"waiting"
    }]}).to_string()).unwrap();
    remote.sandbox.shim("python3", "exit 99");
    let (status, _, body) = remote.request("GET", &format!("/api/conversation/{session}"), Some(&secret), None, false);
    assert_eq!(status, 409, "{body}");
    assert!(remote.sandbox.calls("python3").is_empty());
}

#[test]
fn conversation_access_requires_pairing_and_an_active_session() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.claude_history(&[json!({"type": "user", "message": {"content": "Private"}})]);
    let path = format!("/api/conversation/{SESSION_A}");
    assert_eq!(remote.request("GET", &path, None, None, false).0, 401);
    assert_eq!(remote.request("GET", &path, Some(&secret), None, false).0, 409);
}

#[test]
fn unavailable_conversation_history_is_an_explicit_error() {
    for content in [None, Some("not json\n"), Some("not json")] {
        let remote = Remote::start();
        let secret = remote.paired();
        remote.session_in("waiting", IDLE);
        if let Some(content) = content {
            let path = remote.claude_history(&[]);
            fs::write(path, content).unwrap();
        }
        let (status, _, body) = remote.request("GET", &format!("/api/conversation/{SESSION_A}"), Some(&secret), None, false);
        assert_eq!(status, 409, "{body}");
        assert!(body.contains("history"), "{body}");
    }
}

#[test]
fn conversation_history_keeps_the_latest_hundred_messages() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.session_in("waiting", IDLE);
    remote.claude_history(&(0..105).map(|index| json!({"uuid": format!("u{index}"), "type": "user", "message": {"content": format!("Message {index}")}})).collect::<Vec<_>>());
    let history = remote.get_json(&secret, &format!("/api/conversation/{SESSION_A}"));
    assert_eq!(history["messages"].as_array().unwrap().len(), 100);
    assert_eq!(history["messages"][0]["text"], "Message 5");
    assert_eq!(history["messages"][99]["text"], "Message 104");
    assert_eq!(history["truncated"], true);
}

#[test]
fn conversation_text_is_bounded_without_breaking_unicode() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.session_in("waiting", IDLE);
    remote.claude_history(&[json!({"uuid": "large", "type": "assistant", "message": {"content": [{"type": "text", "text": "é".repeat(200_000)}]}})]);
    let history = remote.get_json(&secret, &format!("/api/conversation/{SESSION_A}"));
    let text = history["messages"][0]["text"].as_str().unwrap();
    assert!(text.len() <= 256 * 1024);
    assert!(!text.contains('\u{fffd}'));
    assert_eq!(history["truncated"], true);
}

#[test]
fn opencode_conversations_use_text_content_and_not_reasoning() {
    let remote = Remote::start();
    let secret = remote.paired();
    fs::write(remote.sandbox.runtime.join("cockpit.json"), json!({"sessions": [{
        "session": OPENCODE_SESSION, "agent": "opencode", "state": "waiting", "tmux_pane": "%5"}]}).to_string()).unwrap();
    let folder = remote.sandbox.home.join(".local/share/opencode");
    fs::create_dir_all(&folder).unwrap();
    let database = rusqlite::Connection::open(folder.join("opencode.db")).unwrap();
    database.execute_batch("create table session_message (id text primary key, session_id text, type text, seq integer, data text);").unwrap();
    for (id, session, role, seq, data) in [
        ("u1", OPENCODE_SESSION, "user", 1, json!({"text": "Fix login"})),
        ("a1", OPENCODE_SESSION, "assistant", 2, json!({"content": [{"type": "reasoning", "text": "private"}, {"type": "text", "text": "Which option?"}]})),
        ("other", "ses_other", "user", 3, json!({"text": "Other session secret"})),
    ] {
        database.execute("insert into session_message values (?1, ?2, ?3, ?4, ?5)", rusqlite::params![id, session, role, seq, data.to_string()]).unwrap();
    }
    assert_eq!(remote.get_json(&secret, &format!("/api/conversation/{OPENCODE_SESSION}")), json!({
        "messages": [{"id": "u1", "role": "user", "text": "Fix login"},
            {"id": "a1", "role": "assistant", "text": "Which option?"}], "truncated": false}));
}

#[test]
fn locked_opencode_history_fails_promptly_without_writing_to_it() {
    let remote = Remote::start();
    let secret = remote.paired();
    fs::write(remote.sandbox.runtime.join("cockpit.json"), json!({"sessions": [{
        "session": OPENCODE_SESSION, "agent": "opencode", "state": "waiting", "tmux_pane": "%5"}]}).to_string()).unwrap();
    let folder = remote.sandbox.home.join(".local/share/opencode");
    fs::create_dir_all(&folder).unwrap();
    let database = rusqlite::Connection::open(folder.join("opencode.db")).unwrap();
    database.execute_batch("create table session_message (id text, session_id text, type text, seq integer, data text); begin exclusive;").unwrap();
    let started = std::time::Instant::now();
    let (status, _, body) = remote.request("GET", &format!("/api/conversation/{OPENCODE_SESSION}"), Some(&secret), None, false);
    assert_eq!(status, 409, "{body}");
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    database.execute_batch("rollback").unwrap();
    assert_eq!(database.query_row("select count(*) from session_message", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
}

#[test]
fn history_does_not_follow_symlinks_or_wait_on_pipes() {
    for is_pipe in [false, true] {
        let remote = Remote::start();
        let secret = remote.paired();
        remote.session_in("waiting", IDLE);
        let path = remote.claude_history(&[]);
        fs::remove_file(&path).unwrap();
        if is_pipe {
            assert!(std::process::Command::new("mkfifo").arg(&path).status().unwrap().success());
        } else {
            let private = remote.sandbox.home.join("private");
            fs::write(&private, "private data").unwrap();
            std::os::unix::fs::symlink(&private, &path).unwrap();
        }
        assert_eq!(remote.request("GET", &format!("/api/conversation/{SESSION_A}"), Some(&secret), None, false).0, 409);
    }
}

#[test]
fn a_partial_streaming_record_does_not_hide_finished_messages() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.session_in("working", IDLE);
    let path = remote.claude_history(&[json!({"uuid": "u1", "type": "user", "message": {"content": "Hello"}})]);
    use std::io::Write;
    fs::OpenOptions::new().append(true).open(path).unwrap().write_all(b"{\"type\":\"assistant\"").unwrap();
    let history = remote.get_json(&secret, &format!("/api/conversation/{SESSION_A}"));
    assert_eq!(history["messages"][0]["text"], "Hello");
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n not really pixels";

impl Remote {
    /// Session A waiting in its folder `checkout`, its last reply `reply`; the folder's path.
    fn session_replying(&self, reply: &str) -> std::path::PathBuf {
        let checkout = self.sandbox.home.join("checkout");
        fs::create_dir_all(checkout.join("design")).unwrap();
        fs::write(self.sandbox.runtime.join("cockpit.json"), json!({"sessions": [{"session": SESSION_A, "state": "waiting",
            "project": "proj-a", "tmux_pane": "%4", "cwd": checkout}]}).to_string()).unwrap();
        self.claude_history(&[json!({"uuid": "u1", "type": "user", "message": {"content": "![mine](design/login.png)"}}),
                              json!({"uuid": "a1", "type": "assistant", "message": {"content": [{"type": "text", "text": reply}]}})]);
        checkout
    }

    /// The status, the headers and the raw body of a GET.
    fn get_bytes(&self, path: &str, cookie: &str) -> (u16, String, Vec<u8>) {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        stream.write_all(format!("GET {path} HTTP/1.0\r\nCookie: claude_remote={cookie}\r\n\r\n").as_bytes()).unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).unwrap();
        let split = response.windows(4).position(|window| window == b"\r\n\r\n").unwrap();
        let headers = String::from_utf8_lossy(&response[..split]).to_string();
        (headers.split_whitespace().nth(1).unwrap().parse().unwrap(), headers, response[split + 4..].to_vec())
    }
}

#[test]
fn an_image_line_in_a_reply_shows_that_file_from_the_sessions_folder() {
    let remote = Remote::start();
    let secret = remote.paired();
    let checkout = remote.session_replying("The login screen:\n![Login](design/login.png)\nWhat do you think?");
    fs::write(checkout.join("design/login.png"), PNG).unwrap();
    let (status, headers, body) = remote.get_bytes(&format!("/api/image/{SESSION_A}/a1/0"), &secret);
    assert_eq!(status, 200, "{headers}");
    assert!(headers.contains("Content-Type: image/png"), "{headers}");
    assert!(headers.contains("X-Content-Type-Options: nosniff"), "{headers}");
    assert_eq!(body, PNG);
}

#[test]
fn an_image_needs_a_paired_phone() {
    let remote = Remote::start();
    let checkout = remote.session_replying("![Login](design/login.png)");
    fs::write(checkout.join("design/login.png"), PNG).unwrap();
    assert_eq!(remote.get_bytes(&format!("/api/image/{SESSION_A}/a1/0"), &"0".repeat(64)).0, 401);
}

#[test]
fn only_an_image_a_reply_shows_from_inside_the_folder_is_served() {
    let cases: &[(&str, &str, &str)] = &[
        ("outside the folder", "![x](../private.png)", "/a1/0"),
        ("an absolute path elsewhere", "![x](/etc/hostname)", "/a1/0"),
        ("a symlink out of the folder", "![x](design/link.png)", "/a1/0"),
        ("not an image", "![x](design/notes.txt)", "/a1/0"),
        ("SVG, whose markup could run", "![x](design/icon.svg)", "/a1/0"),
        ("a folder", "![x](design)", "/a1/0"),
        ("an image the reply does not show", "![x](design/login.png)", "/a1/1"),
        ("an image line inside fenced code", "```\n![x](design/login.png)\n```", "/a1/0"),
        ("an image line in your own message", "No image here", "/u1/0"),
        ("a reply that is not in the conversation", "![x](design/login.png)", "/gone/0"),
    ];
    for (what, reply, address) in cases {
        let remote = Remote::start();
        let secret = remote.paired();
        let checkout = remote.session_replying(reply);
        fs::write(checkout.join("design/login.png"), PNG).unwrap();
        fs::write(checkout.join("design/notes.txt"), "private notes").unwrap();
        fs::write(checkout.join("design/icon.svg"), "<svg onload=alert(1)/>").unwrap();
        fs::write(remote.sandbox.home.join("private.png"), PNG).unwrap();
        std::os::unix::fs::symlink(remote.sandbox.home.join("private.png"), checkout.join("design/link.png")).unwrap();
        let (status, headers, body) = remote.get_bytes(&format!("/api/image/{SESSION_A}{address}"), &secret);
        assert_eq!(status, 404, "{what}: {headers}");
        assert!(!body.starts_with(b"\x89PNG"), "{what}");
    }
}

#[test]
fn an_image_larger_than_ten_megabytes_is_refused() {
    let remote = Remote::start();
    let secret = remote.paired();
    let checkout = remote.session_replying("![Big](design/big.png)");
    fs::write(checkout.join("design/big.png"), [PNG, &vec![0; 10 * 1024 * 1024]].concat()).unwrap();
    let (status, _, body) = remote.get_bytes(&format!("/api/image/{SESSION_A}/a1/0"), &secret);
    assert_eq!(status, 404);
    assert!(String::from_utf8_lossy(&body).contains("larger than 10 MB"));
}

fn subscription(endpoint: &str) -> Value {
    json!({"endpoint": endpoint, "keys": {
        "p256dh": "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4", "auth": "BTBZMqHH6r4Tts7J_aSIgg"}})
}

fn takes_notifications(remote: &Remote, secret: &str) -> bool {
    let (_, _, body) = remote.request("GET", "/api/state", Some(secret), None, false);
    serde_json::from_str::<Value>(&body).unwrap()["push"].as_bool().unwrap()
}

#[test]
fn a_phone_turns_its_notifications_on_and_off() {
    let remote = Remote::start();
    let secret = remote.paired();
    let (status, _, body) = remote.request("POST", "/api/push/subscribe", Some(&secret), Some(subscription("https://web.push.apple.com/QGuQ")), true);
    assert_eq!(status, 200, "{body}");
    assert!(takes_notifications(&remote, &secret));
    remote.request("POST", "/api/push/unsubscribe", Some(&secret), Some(json!({})), true);
    assert!(!takes_notifications(&remote, &secret));
}

#[test]
fn an_existing_phone_keeps_its_cookie_and_subscription_across_the_path_migration() {
    let mut remote = Remote::start();
    let old = remote.sandbox.state.join("claude-remote");
    let new = remote.sandbox.state.join("cockpit-remote");
    fs::remove_file(&old).unwrap();
    fs::rename(&new, &old).unwrap();
    remote.restart();
    let secret = remote.paired();
    let (status, _, body) = remote.request("POST", "/api/push/subscribe", Some(&secret),
        Some(subscription("https://web.push.apple.com/QGuQ")), true);
    assert_eq!(status, 200, "{body}");
    let before = fs::read(old.join("devices.json")).unwrap();
    let migration = remote.sandbox.run("python3", &[repo().join("tools/migrate-cockpit-names.py").to_str().unwrap(), "--apply"], &[]);
    assert!(migration.status.success(), "{}", stderr(&migration));
    assert_eq!(fs::read(new.join("devices.json")).unwrap(), before);
    remote.restart();
    let (status, _, body) = remote.request("GET", "/api/state", Some(&secret), None, false);
    assert_eq!(status, 200, "{body}");
    assert!(serde_json::from_str::<Value>(&body).unwrap()["push"].as_bool().unwrap());
}

#[test]
fn a_subscription_to_an_unknown_host_is_refused() {
    let remote = Remote::start();
    let secret = remote.paired();
    let (status, _, body) = remote.request("POST", "/api/push/subscribe", Some(&secret), Some(subscription("https://192.168.1.1/admin")), true);
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("not a known push service"), "{body}");
    assert!(!takes_notifications(&remote, &secret));
}

#[test]
fn the_push_key_is_a_p256_point_that_stays_the_same() {
    let remote = Remote::start();
    let secret = remote.paired();
    let key = |remote: &Remote| serde_json::from_str::<Value>(&remote.request("GET", "/api/push/key", Some(&secret), None, false).2).unwrap()["key"].as_str().unwrap().to_string();
    let first = key(&remote);
    assert_eq!(first.len(), 87, "{first}"); // 65 bytes in base64url
    assert_eq!(key(&remote), first);
}

#[test]
fn a_paired_phone_gets_a_code_that_pairs_its_home_screen_app_once() {
    let remote = Remote::start();
    let secret = remote.paired_as(&["iPhone"]);
    let (status, _, body) = remote.request("POST", "/api/pairing-code", Some(&secret), Some(json!({})), true);
    assert_eq!(status, 200, "{body}");
    let code = serde_json::from_str::<Value>(&body).unwrap()["message"].as_str().unwrap().to_string();
    assert_eq!(remote.request("GET", &format!("/pair/{code}"), None, None, false).0, 303);
    assert_eq!(remote.request("GET", &format!("/pair/{code}"), None, None, false).0, 403, "used twice");
    assert!(remote.remote(&["phones"]).contains("iPhone (Home Screen)"));
}

#[test]
fn yes_and_no_refuse_an_opencode_session() {
    // Its permission questions are not Claude's yes/no menu: Enter or Escape could pick another answer.
    for (path, state, body) in [("answer", "needs-you", json!({"yes": true}))] {
        let remote = Remote::start();
        let secret = remote.paired();
        fs::write(remote.sandbox.runtime.join("cockpit.json"),
                  json!({"sessions": [{"session": SESSION_A, "state": state, "project": "proj-a", "tmux_pane": "%4", "agent": "opencode"}]}).to_string()).unwrap();
        fs::write(remote.sandbox.runtime.join("pane.txt"), DIALOG).unwrap();
        let (status, _, answered) = remote.request("POST", &format!("/api/{path}/{SESSION_A}"), Some(&secret), Some(body), true);
        assert_eq!(status, 409, "{path}: {answered}");
        assert!(answered.contains("runs opencode"), "{path}: {answered}");
        assert!(remote.keys_pressed().is_empty(), "{path}");
    }
}

#[test]
fn a_phone_answers_an_opencode_chat_question_through_the_real_manager() {
    let remote = Remote::start();
    let secret = remote.paired();
    fs::write(remote.sandbox.runtime.join("cockpit.json"), json!({"sessions": [{
        "session": OPENCODE_SESSION, "agent": "opencode", "state": "needs-you", "needs_input": true,
        "project": "proj-a", "tmux_pane": "%5"}]}).to_string()).unwrap();
    let (status, _, body) = remote.request("POST", &format!("/api/message/{OPENCODE_SESSION}"),
        Some(&secret), Some(json!({"text": "Use option A"})), true);
    assert_eq!(status, 200, "{body}");
    assert!(remote.sandbox.calls("tmux").contains(&"tmux send-keys -t %5 -l -- Use option A".into()));
}

// ---- Work from the phone ----

#[test]
fn multiline_phone_replies_reach_each_agents_paste_unchanged() {
    for (agent, session, pane) in [("claude", SESSION_A, "%4"), ("opencode", OPENCODE_SESSION, "%5")] {
        let remote = Remote::start();
        let secret = remote.paired();
        fs::write(remote.sandbox.runtime.join("cockpit.json"), json!({"sessions": [{
            "session": session, "agent": agent, "state": "waiting", "project": "proj-a", "tmux_pane": pane
        }]}).to_string()).unwrap();
        fs::write(remote.sandbox.runtime.join("pane.txt"), IDLE).unwrap();
        remote.sandbox.shim("tmux", r#"case $1 in
            capture-pane) cat "$XDG_RUNTIME_DIR/pane.txt" ;;
            load-buffer) cat > "$XDG_RUNTIME_DIR/message-bytes" ;;
        esac
        exit 0"#);
        let (status, answer) = remote.post_json(&secret, &format!("/api/message/{session}"),
            json!({"text": "First line\r\n\r\n  - second line"}));
        assert_eq!(status, 200, "{agent}: {answer}");
        assert_eq!(fs::read_to_string(remote.sandbox.runtime.join("message-bytes")).unwrap(), "First line\n\n  - second line");
        assert_eq!(remote.keys_pressed(), [format!("tmux send-keys -t {pane} Enter")]);
    }
}

#[test]
fn multiline_phone_replies_wait_unchanged_for_the_end_of_the_turn() {
    for (agent, session) in [("claude", SESSION_A), ("opencode", OPENCODE_SESSION)] {
        let remote = Remote::start();
        let secret = remote.paired();
        fs::write(remote.sandbox.runtime.join("cockpit.json"), json!({"sessions": [{
            "session": session, "agent": agent, "state": "working", "project": "proj-a", "tmux_pane": "%4"
        }]}).to_string()).unwrap();
        let (status, answer) = remote.post_json(&secret, &format!("/api/message/{session}"),
            json!({"text": "First line\r\n\r\n  - second line"}));
        assert_eq!(status, 200, "{agent}: {answer}");
        let files: Vec<_> = fs::read_dir(remote.sandbox.state.join("cockpit/prompt-queue").join(session)).unwrap().flatten().collect();
        assert_eq!(files.len(), 1);
        assert_eq!(fs::read_to_string(files[0].path()).unwrap(), "First line\n\n  - second line");
        assert!(remote.keys_pressed().is_empty());
    }
}

#[test]
fn multiline_first_messages_remain_one_argument_for_both_agents_and_launch_modes() {
    for agent in ["claude", "opencode"] {
        for in_clone in [false, true] {
            let remote = Remote::start();
            let secret = remote.paired();
            remote.sandbox.shim("cockpit", r#"printf '%s\0' "$@" > "$XDG_RUNTIME_DIR/launch-arguments""#);
            let directory = remote.project("shop");
            let (status, answer) = remote.post_json(&secret, "/api/sessions", json!({
                "project": directory, "agent": agent, "prompt": "First line\r\n\r\n  - second line", "clone": in_clone
            }));
            assert_eq!(status, 200, "{agent}/{in_clone}: {answer}");
            let bytes = fs::read(remote.sandbox.runtime.join("launch-arguments")).unwrap();
            let arguments: Vec<_> = bytes.split(|byte| *byte == 0).filter(|part| !part.is_empty()).collect();
            assert_eq!(arguments.last().copied(), Some(b"First line\n\n  - second line".as_slice()));
            assert_eq!(arguments[2], if in_clone { b"parallel".as_slice() } else { b"background".as_slice() });
        }
    }
}

#[test]
fn phone_messages_and_first_prompts_still_reject_other_controls_and_invalid_lengths() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.session_in("working", IDLE);
    remote.sandbox.shim("cockpit", "exit 0");
    let directory = remote.project("shop");
    for text in ["".to_string(), " \n ".into(), "a\n\u{1b}[201~b".into(), "a\n\u{3}b".into(), "a\tb".into(), "a\rb".into(), "é".repeat(4001)] {
        let (status, answer) = remote.post_json(&secret, &format!("/api/message/{SESSION_A}"), json!({"text": text}));
        assert_eq!(status, 409, "{answer}");
        let (status, answer) = remote.post_json(&secret, "/api/sessions", json!({"project": directory, "agent": "claude", "prompt": text}));
        assert_eq!(status, 409, "{answer}");
    }
    assert!(!remote.sandbox.state.join("cockpit/prompt-queue").exists());
    assert!(remote.sandbox.calls("cockpit").is_empty());
    let valid = format!("{}\né", "é".repeat(3998));
    assert_eq!(remote.post_json(&secret, &format!("/api/message/{SESSION_A}"), json!({"text": valid})).0, 200);
}

#[test]
fn allowing_paragraphs_in_messages_does_not_allow_multiline_todo_titles() {
    let remote = Remote::start();
    let secret = remote.paired();
    let directory = remote.project("shop");
    let (status, answer) = remote.post_json(&secret, "/api/todos", json!({"project": directory, "text": "First\nsecond", "kind": "bug"}));
    assert_eq!(status, 409, "{answer}");
    assert!(remote.todo_updates().is_empty());
}

impl Remote {
    /// A project folder under ~/IdeaProjects, which the column's picker lists.
    fn project(&self, name: &str) -> String {
        let folder = self.sandbox.home.join("IdeaProjects").join(name);
        fs::create_dir_all(&folder).unwrap();
        folder.display().to_string()
    }

    fn post_json(&self, secret: &str, path: &str, body: Value) -> (u16, Value) {
        let (status, _, answered) = self.request("POST", path, Some(secret), Some(body), true);
        (status, serde_json::from_str(&answered).unwrap_or(json!({"raw": answered})))
    }

    fn get_json(&self, secret: &str, path: &str) -> Value {
        let (status, _, answered) = self.request("GET", path, Some(secret), None, false);
        assert_eq!(status, 200, "{path}: {answered}");
        serde_json::from_str(&answered).unwrap()
    }

    fn todo_updates(&self) -> Vec<Value> {
        fs::read_to_string(self.sandbox.runtime.join("cockpit-todo-updates.jsonl")).unwrap_or_default().lines()
            .map(|line| serde_json::from_str(line).unwrap()).collect()
    }
}

#[test]
fn a_new_session_starts_out_of_sight_in_its_own_clone_or_in_the_folder() {
    for (is_in_clone, agent, expected) in [(true, "claude", "cockpit --agent claude parallel {dir} phone-"),
                                           (false, "claude", "cockpit --agent claude background {dir} Fix the login page"),
                                           (true, "opencode", "cockpit --agent opencode parallel {dir} phone-"),
                                           (false, "opencode", "cockpit --agent opencode background {dir} Fix the login page")] {
        let remote = Remote::start();
        let secret = remote.paired();
        remote.sandbox.shim("cockpit", "exit 0");
        let directory = remote.project("shop");
        let (status, answer) = remote.post_json(&secret, "/api/sessions", json!({"project": directory, "agent": agent, "prompt": "Fix the login page", "clone": is_in_clone}));
        assert_eq!(status, 200, "{answer}");
        let call = remote.sandbox.calls("cockpit").pop().unwrap();
        assert!(call.starts_with(&expected.replace("{dir}", &directory)), "{call}");
        assert!(call.ends_with("Fix the login page"), "{call}");
    }
}

#[test]
fn a_session_starts_only_in_a_listed_project() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.sandbox.shim("cockpit", "exit 0");
    let (status, answer) = remote.post_json(&secret, "/api/sessions", json!({"project": "/etc", "agent": "claude", "prompt": "hi"}));
    assert_eq!(status, 409, "{answer}");
    assert!(remote.sandbox.calls("cockpit").is_empty());
}

#[test]
fn a_session_starts_only_with_an_agent_the_column_offers() {
    for body in [json!({"agent": "unknown-agent"}), json!({})] {
        let remote = Remote::start();
        let secret = remote.paired();
        remote.sandbox.shim("cockpit", "exit 0");
        let mut body = body;
        body["project"] = json!(remote.project("shop"));
        body["prompt"] = json!("hi");
        let (status, answer) = remote.post_json(&secret, "/api/sessions", body);
        assert_eq!(status, 409, "{answer}");
        assert!(answer.to_string().contains("which agent?"), "{answer}");
        assert!(remote.sandbox.calls("cockpit").is_empty());
    }
}

#[test]
fn the_projects_are_the_pickers() {
    let remote = Remote::start();
    let secret = remote.paired();
    let directory = remote.project("shop");
    let projects = remote.get_json(&secret, "/api/projects");
    assert!(projects.as_array().unwrap().iter().any(|project| project["path"] == directory.as_str() && project["name"] == "shop"), "{projects}");
}

#[test]
fn an_item_added_from_the_phone_goes_through_the_columns_update_queue() {
    let remote = Remote::start();
    let secret = remote.paired();
    let directory = remote.project("shop");
    let (status, answer) = remote.post_json(&secret, "/api/todos", json!({"project": directory, "text": "Cache the cart", "notes": "see #12"}));
    assert_eq!(status, 200, "{answer}");
    let updates = remote.todo_updates();
    assert_eq!((updates[0]["action"].as_str(), updates[0]["started_in"].as_str(), updates[0]["text"].as_str()),
               (Some("add"), Some(directory.as_str()), Some("Cache the cart")));
    assert!(!remote.sandbox.state.join("cockpit/todo.json").exists(), "the phone wrote the column's file");
}

#[test]
fn the_open_items_are_listed_and_done_ones_are_not() {
    let remote = Remote::start();
    let secret = remote.paired();
    let todo = remote.sandbox.state.join("cockpit/todo.json");
    fs::create_dir_all(todo.parent().unwrap()).unwrap();
    fs::write(&todo, json!({"items": [
        {"id": "aaaa1111", "project": "shop", "cwd": "/x", "text": "open one", "status": "todo", "created": 1},
        {"id": "bbbb2222", "project": "shop", "cwd": "/x", "text": "finished", "status": "done", "created": 2}]}).to_string()).unwrap();
    let todos = remote.get_json(&secret, "/api/todos");
    let texts: Vec<&str> = todos.as_array().unwrap().iter().map(|item| item["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["open one"]);
}

#[test]
fn starting_an_item_hands_it_to_a_new_clone_and_marks_it_in_progress() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.sandbox.shim("cockpit", "exit 0");
    let directory = remote.project("shop");
    let todo = remote.sandbox.state.join("cockpit/todo.json");
    fs::create_dir_all(todo.parent().unwrap()).unwrap();
    fs::write(&todo, json!({"items": [{"id": "aaaa1111", "project": "shop", "cwd": directory, "text": "Cache the cart", "status": "todo", "created": 1}]}).to_string()).unwrap();
    let host = remote.sandbox.runtime.join("claude-team-host/host.sock");
    fs::create_dir_all(host.parent().unwrap()).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(host).unwrap();
    let arbitration = std::thread::spawn(move || {
        let mut requests=Vec::new();
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().unwrap();
            let mut text = String::new();stream.read_to_string(&mut text).unwrap();
            requests.push(serde_json::from_str::<Value>(&text).unwrap());
            stream.write_all(br#"{"request_id":"legacy","result":{"Ok":{"reservation":"reserved-before-spawn"}}}"#).unwrap();
        }
        requests
    });
    let (status, answer) = remote.post_json(&secret, "/api/todos/aaaa1111/start", json!({"agent": "opencode"}));
    assert_eq!(status, 200, "{answer}");
    let requests = arbitration.join().unwrap();
    let reserved=&requests[0];
    assert_eq!(requests[1]["outcome"],"begin");assert_eq!(requests[1]["token"],"reserved-before-spawn");
    assert_eq!(requests[2]["outcome"],"submitted");assert_eq!(requests[2]["expected_revision"],2);
    assert_eq!(reserved["request"], "reserve_task");
    assert_eq!(reserved["task_id"], "aaaa1111");
    assert_eq!(reserved["directory"], directory);
    let call = remote.sandbox.calls("cockpit").pop().unwrap();
    assert!(call.starts_with(&format!("cockpit --agent opencode parallel {directory} todo-aaaa1111 Cache the cart (todo aaaa1111")), "{call}");
    let updates = remote.todo_updates();
    assert_eq!((updates[0]["action"].as_str(), updates[0]["id"].as_str()), (Some("doing"), Some("aaaa1111")));
}

#[test]
fn starting_an_item_from_phone_fails_closed_when_host_arbitration_is_missing() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.sandbox.shim("cockpit", "exit 0");
    let directory = remote.project("shop");
    let todo = remote.sandbox.state.join("cockpit/todo.json");
    fs::create_dir_all(todo.parent().unwrap()).unwrap();
    fs::write(todo, json!({"items":[{"id":"aaaa1111","project":"shop","cwd":directory,"text":"request","status":"todo"}]}).to_string()).unwrap();
    let (status, answer) = remote.post_json(&secret, "/api/todos/aaaa1111/start", json!({"agent":"opencode"}));
    assert_ne!(status, 200, "{answer}");
    assert!(remote.sandbox.calls("cockpit").is_empty());
    assert!(remote.todo_updates().is_empty());
}

#[test]
fn an_item_added_from_the_phone_keeps_its_kind() {
    let remote = Remote::start();
    let secret = remote.paired();
    let directory = remote.project("shop");
    let (status, answer) = remote.post_json(&secret, "/api/todos", json!({"project": directory, "text": "Cart total", "kind": "bug"}));
    assert_eq!(status, 200, "{answer}");
    assert_eq!(remote.todo_updates()[0]["category"].as_str(), Some("bug"));
}

#[test]
fn an_unknown_kind_is_refused() {
    let remote = Remote::start();
    let secret = remote.paired();
    let directory = remote.project("shop");
    let (status, answer) = remote.post_json(&secret, "/api/todos", json!({"project": directory, "text": "Cart total", "kind": "epic"}));
    assert_ne!(status, 200, "{answer}");
    assert!(remote.todo_updates().is_empty());
}

#[test]
fn the_listed_items_carry_their_kind_and_queue() {
    let remote = Remote::start();
    let secret = remote.paired();
    let todo = remote.sandbox.state.join("cockpit/todo.json");
    fs::create_dir_all(todo.parent().unwrap()).unwrap();
    fs::write(&todo, json!({"items": [{"id": "aaaa1111", "project": "shop", "cwd": "/x", "text": "open one", "status": "todo",
                                       "category": "bug", "created": 1}], "queued": ["aaaa1111"]}).to_string()).unwrap();
    let todos = remote.get_json(&secret, "/api/todos");
    assert_eq!((todos[0]["kind"].as_str(), todos[0]["is_queued"].as_bool()), (Some("Bug"), Some(true)), "{todos}");
}

#[test]
fn marking_an_item_done_goes_through_the_columns_update_queue() {
    let remote = Remote::start();
    let secret = remote.paired();
    let directory = remote.project("shop");
    let todo = remote.sandbox.state.join("cockpit/todo.json");
    fs::create_dir_all(todo.parent().unwrap()).unwrap();
    fs::write(&todo, json!({"items": [{"id": "aaaa1111", "project": "shop", "cwd": directory, "text": "Cache the cart", "status": "doing", "created": 1}]}).to_string()).unwrap();
    let (status, answer) = remote.post_json(&secret, "/api/todos/aaaa1111/done", json!({}));
    assert_eq!(status, 200, "{answer}");
    let updates = remote.todo_updates();
    assert_eq!((updates[0]["action"].as_str(), updates[0]["id"].as_str(), updates[0]["started_in"].as_str()),
               (Some("done"), Some("aaaa1111"), Some(directory.as_str())));
}

#[test]
fn a_done_item_cannot_be_marked_done_again() {
    let remote = Remote::start();
    let secret = remote.paired();
    let todo = remote.sandbox.state.join("cockpit/todo.json");
    fs::create_dir_all(todo.parent().unwrap()).unwrap();
    fs::write(&todo, json!({"items": [{"id": "aaaa1111", "project": "shop", "cwd": "/x", "text": "finished", "status": "done", "created": 1}]}).to_string()).unwrap();
    let (status, answer) = remote.post_json(&secret, "/api/todos/aaaa1111/done", json!({}));
    assert_ne!(status, 200, "{answer}");
    assert!(remote.todo_updates().is_empty());
}

#[test]
fn the_usage_reads_the_columns_cache() {
    let remote = Remote::start();
    let secret = remote.paired();
    fs::write(remote.sandbox.runtime.join("claude-usage.json"),
              json!({"five_hour": {"utilization": 42.4, "resets_at": "2099-01-01T00:00:00Z"}}).to_string()).unwrap();
    let usage = remote.get_json(&secret, "/api/usage");
    assert_eq!((usage["windows"][0]["label"].as_str(), usage["windows"][0]["percentage"].as_i64()), (Some("Current session"), Some(42)));
}

#[test]
fn the_pull_requests_come_from_gh_and_are_kept_five_minutes() {
    let remote = Remote::start();
    let secret = remote.paired();
    let updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    remote.sandbox.shim("gh", &format!(r#"pull='{{"number": 7, "title": "Fix cart", "url": "https://github.com/o/shop/pull/7", "repository": {{"name": "shop", "nameWithOwner": "o/shop"}}, "author": {{"login": "sam"}}, "updatedAt": "{updated}", "headRefName": "fix-cart", "headRefOid": "abc123"}}'
case "$1 $2" in
  "search prs") echo "[$pull]" ;;
  "pr view") echo "$pull" ;;
  *) exit 1 ;;
esac"#));
    let pulls = remote.get_json(&secret, "/api/pulls");
    assert_eq!(pulls["pulls"][0]["title"], "Fix cart");
    assert!(pulls.get("error").is_none(), "{pulls}");
    let fetch_calls = remote.sandbox.calls("gh").len();
    assert_eq!(fetch_calls, 2, "one search and one metadata view: {:?}", remote.sandbox.calls("gh"));
    remote.get_json(&secret, "/api/pulls");
    assert_eq!(remote.sandbox.calls("gh").len(), fetch_calls, "fetched again within 5 minutes");
    remote.get_json(&secret, "/api/pulls?refresh=1");
    assert_eq!(remote.sandbox.calls("gh").len(), 2 * fetch_calls);
}

#[test]
fn a_pull_request_without_metadata_is_still_listed_and_cached() {
    let remote = Remote::start();
    let secret = remote.paired();
    let updated = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    remote.sandbox.shim("gh", &format!(r#"case "$1 $2" in
  "search prs") echo '[{{"number": 7, "title": "Fix cart", "url": "https://github.com/o/shop/pull/7", "repository": {{"name": "shop", "nameWithOwner": "o/shop"}}, "author": {{"login": "sam"}}, "updatedAt": "{updated}"}}]' ;;
  *) echo "GraphQL: Resource protected by organization SAML enforcement" >&2; exit 1 ;;
esac"#));
    let pulls = remote.get_json(&secret, "/api/pulls");
    assert_eq!(pulls["pulls"][0]["title"], "Fix cart");
    assert!(pulls.get("error").is_none(), "{pulls}");
    let fetch_calls = remote.sandbox.calls("gh").len();
    remote.get_json(&secret, "/api/pulls");
    assert_eq!(remote.sandbox.calls("gh").len(), fetch_calls, "fetched again within 5 minutes");
}

#[test]
fn a_phone_watches_a_session_and_stops() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.session_in("working", IDLE);
    let (status, answer) = remote.post_json(&secret, &format!("/api/watch/{SESSION_A}"), json!({"on": true}));
    assert_eq!(status, 200, "{answer}");
    assert_eq!(remote.get_json(&secret, "/api/state")["watched"], json!([SESSION_A]));
    remote.post_json(&secret, &format!("/api/watch/{SESSION_A}"), json!({"on": false}));
    assert_eq!(remote.get_json(&secret, "/api/state")["watched"], json!([]));
}

// ---- Asleep sessions ----

impl Remote {
    /// A sleep note (sleep::Asleep), with no row in the snapshot: the session's process is gone.
    fn asleep_note(&self, id: &str, slept_at: i64) {
        let folder = self.sandbox.state.join("cockpit/asleep");
        fs::create_dir_all(&folder).unwrap();
        fs::write(folder.join(id), json!({
            "agent": "claude", "project": "proj-a", "cwd": "/p", "title": "Fix the build",
            "idle_since": slept_at - 3600, "slept_at": slept_at, "remote": false,
        }).to_string()).unwrap();
    }
}

#[test]
fn phone_lists_asleep_sessions() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.asleep_note(SESSION_A, 1_700_000_000);
    let cards = remote.get_json(&secret, "/api/state");
    let card = cards["sessions"].as_array().unwrap().iter().find(|card| card["session"] == SESSION_A).unwrap_or_else(|| panic!("{cards}"));
    assert_eq!((card["state"].clone(), card["since"].clone(), card["project"].clone(), card["task"].clone()),
               (json!("asleep"), json!(1_700_000_000), json!("proj-a"), json!("Fix the build")));
}

#[test]
fn phone_lists_a_session_once_when_its_row_and_its_note_both_exist() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.session_in("waiting", IDLE);
    remote.asleep_note(SESSION_A, 1_700_000_000);
    let cards = remote.get_json(&secret, "/api/state");
    assert_eq!(cards["sessions"].as_array().unwrap().len(), 1, "{cards}");
    assert_eq!(cards["sessions"][0]["state"], "waiting");
}

#[test]
fn phone_wake_starts_cockpit_wake() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.sandbox.shim("cockpit", "exit 0");
    remote.asleep_note(SESSION_A, 1_700_000_000);
    let (status, answer) = remote.post_json(&secret, &format!("/api/wake/{SESSION_A}"), json!({}));
    assert_eq!((status, answer["message"].clone()), (200, json!("waking")), "{answer}");
    assert_eq!(remote.sandbox.wait_for_calls("cockpit", 1, std::time::Duration::from_secs(5)), [format!("cockpit wake {SESSION_A}")]);
}

#[test]
fn phone_wake_refuses_an_unknown_session() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.sandbox.shim("cockpit", "exit 0");
    let (status, answer) = remote.post_json(&secret, &format!("/api/wake/{SESSION_A}"), json!({}));
    assert_eq!((status, answer["message"].clone()), (409, json!("no such session")), "{answer}");
    assert_eq!(remote.sandbox.calls("cockpit"), Vec::<String>::new());
}

#[test]
fn phone_wake_takes_an_opencode_session_id() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.sandbox.shim("cockpit", "exit 0");
    remote.asleep_note(OPENCODE_SESSION, 1_700_000_000);
    let (status, answer) = remote.post_json(&secret, &format!("/api/wake/{OPENCODE_SESSION}"), json!({}));
    assert_eq!(status, 200, "{answer}");
    assert_eq!(remote.sandbox.wait_for_calls("cockpit", 1, std::time::Duration::from_secs(5)), [format!("cockpit wake {OPENCODE_SESSION}")]);
}

#[test]
fn phone_message_to_an_asleep_session_is_queued_and_wakes_it() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.sandbox.shim("cockpit", "exit 0");
    remote.asleep_note(SESSION_A, 1_700_000_000);
    let (status, answer) = remote.post_json(&secret, &format!("/api/message/{SESSION_A}"), json!({"text": "carry on"}));
    assert_eq!((status, answer["message"].clone()), (200, json!("queued; it wakes now")), "{answer}");
    assert_eq!(remote.sandbox.calls("cockpit"), [format!("cockpit say {SESSION_A} carry on")]);
}

#[test]
fn phone_message_to_an_asleep_session_that_cannot_wake_says_why() {
    let remote = Remote::start();
    let secret = remote.paired();
    remote.sandbox.shim("cockpit", "echo 'its folder is gone' >&2; exit 1");
    remote.asleep_note(SESSION_A, 1_700_000_000);
    let (status, answer) = remote.post_json(&secret, &format!("/api/message/{SESSION_A}"), json!({"text": "carry on"}));
    assert_eq!((status, answer["message"].clone()), (409, json!("its folder is gone")), "{answer}");
}

// ---- Pairing with 6 digits (the Home Screen app) ----

impl Remote {
    /// The 6 digits `claude-remote pair` shows.
    fn pairing_digits(&self) -> String {
        let shown = self.remote(&["pair"]);
        shown.split("within 2 minutes: ").nth(1).map(|rest| rest.chars().filter(char::is_ascii_digit).take(6).collect())
            .unwrap_or_else(|| panic!("no digits in {shown}"))
    }

    fn type_digits(&self, digits: &str, from_page: bool) -> (u16, String, String) {
        self.request("POST", "/pair-code", None, Some(json!({"code": digits})), from_page)
    }
}

fn wrong(digits: &str) -> String {
    format!("{:06}", (digits.parse::<u32>().unwrap() + 1) % 1_000_000)
}

#[test]
fn the_six_digits_pair_the_home_screen_app_once() {
    let remote = Remote::start();
    let digits = remote.pairing_digits();
    let (status, headers, body) = remote.type_digits(&format!("{} {}", &digits[..3], &digits[3..]), true);
    assert_eq!(status, 200, "{body}");
    let secret = headers.lines().find_map(|line| line.strip_prefix("Set-Cookie: claude_remote=")).unwrap().split(';').next().unwrap().to_string();
    assert_eq!(remote.request("GET", "/api/state", Some(&secret), None, false).0, 200);
    assert_eq!(remote.type_digits(&digits, true).0, 403, "used twice");
}

#[test]
fn five_wrong_codes_cancel_the_digits() {
    let remote = Remote::start();
    let digits = remote.pairing_digits();
    for tries_left in (1..5).rev() {
        let (status, _, body) = remote.type_digits(&wrong(&digits), true);
        assert_eq!(status, 403);
        assert!(body.contains(&format!("{tries_left} tries left")), "{body}");
    }
    let (_, _, body) = remote.type_digits(&wrong(&digits), true);
    assert!(body.contains("cancelled"), "{body}");
    assert_eq!(remote.type_digits(&digits, true).0, 403, "the right code after the fifth wrong one");
}

#[test]
fn the_digits_expire_after_two_minutes() {
    let remote = Remote::start();
    let digits = remote.pairing_digits();
    let file = remote.sandbox.state.join("claude-remote/pairing.json");
    let mut pairing: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    pairing["digits_until"] = json!(1);
    fs::write(&file, pairing.to_string()).unwrap();
    let (status, _, body) = remote.type_digits(&digits, true);
    assert_eq!(status, 403);
    assert!(body.contains("expired"), "{body}");
}

#[test]
fn the_digits_need_the_pages_header() {
    let remote = Remote::start();
    let digits = remote.pairing_digits();
    assert_eq!(remote.type_digits(&digits, false).0, 403);
    assert_eq!(remote.type_digits(&digits, true).0, 200, "the refused post cost nothing");
}
