//! A confined run's way to root (#1387): `torlek-sandbox root-route-sudo`, the run's `sudo`,
//! hands its command to `torlek-sandbox run-root-route`, which asks the daemon (this test)
//! and runs what it lets go through sudo. A fake sudo and a fake helper stand in for root
//! here; the helper's own effects are `tests/root_run.rs`'s.
#![cfg(target_os = "linux")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Output, Stdio};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_torlek-sandbox");

fn hex(word: &str) -> String {
    word.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(word: &str) -> String {
    let bytes: Vec<u8> = (0..word.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&word[at..at + 2], 16).unwrap())
        .collect();
    String::from_utf8(bytes).unwrap()
}

fn executable(path: &Path, script: &str) {
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Route {
    dir: tempfile::TempDir,
    relay: Child,
    decisions: ChildStdin,
    asks: BufReader<ChildStdout>,
}

impl Route {
    /// A relay whose sudo prints to its standard error, logs its arguments and askpass, then
    /// runs the helper; or, with `sudo_fails`, runs nothing and exits 1, as after three wrong
    /// passwords.
    fn start(sudo_fails: bool) -> Route {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("sudo.log");
        let sudo = dir.path().join("sudo");
        let run_helper = if sudo_fails {
            "exit 1"
        } else {
            "shift 5\nexec \"$@\""
        };
        executable(
            &sudo,
            &format!(
                "#!/bin/sh\necho 'sudo says hello' >&2\n\
                 {{ for word in \"$@\"; do printf '%s\\n' \"$word\"; done; \
                 printf 'SUDO_ASKPASS=%s\\n' \"$SUDO_ASKPASS\"; }} >'{}'\n{run_helper}\n",
                log.display()
            ),
        );
        let request = dir.path().join("request");
        executable(
            &dir.path().join("helper"),
            &format!(
                "#!/bin/sh\ndd bs=1 count=1 of=/dev/null 2>/dev/null\nread -r request\n\
                 printf '%s\\n' \"$request\" >'{}'\necho 'started u' >&0\necho 'exited 7' >&0\n",
                request.display()
            ),
        );
        let folder = dir.path().join("route");
        std::fs::create_dir(&folder).unwrap();
        let mut relay = Command::new(LAUNCHER)
            .arg("run-root-route")
            .arg(&folder)
            .arg(&sudo)
            .args(["/the/askpass", "42", "xterm-256color"])
            .args(["--write", "/srv/site", "--hide", "/home/o/.config/secret"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let decisions = relay.stdin.take().unwrap();
        let mut asks = BufReader::new(relay.stdout.take().unwrap());
        let mut ready = String::new();
        asks.read_line(&mut ready).unwrap();
        assert_eq!(ready, "ready\n");
        Route {
            dir,
            relay,
            decisions,
            asks,
        }
    }

    fn folder(&self) -> PathBuf {
        self.dir.path().join("route")
    }

    fn helper(&self) -> PathBuf {
        self.dir.path().join("helper")
    }

    /// The run's `sudo ARGUMENTS`, started in the test's folder.
    fn sudo(&self, arguments: &[&str]) -> Child {
        Command::new(LAUNCHER)
            .arg("root-route-sudo")
            .arg(self.folder())
            .args(arguments)
            .current_dir(self.dir.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    /// The relay's next line, split at spaces.
    fn next(&mut self) -> Vec<String> {
        let mut line = String::new();
        self.asks.read_line(&mut line).unwrap();
        line.trim_end().split(' ').map(str::to_owned).collect()
    }

    fn decide(&mut self, line: &str) {
        writeln!(self.decisions, "{line}").unwrap();
    }

    fn file(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(name)).unwrap_or_default()
    }
}

impl Drop for Route {
    fn drop(&mut self) {
        let _ = self.relay.kill();
        let _ = self.relay.wait();
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The run's `sudo true x`, let go: what it exited with and printed.
fn let_go(route: &mut Route) -> Output {
    let sudo = route.sudo(&["-A", "-k", "true", "x"]);
    let ask = route.next();
    assert_eq!(&ask[..2], ["ask", "1"], "{ask:?}");
    assert_eq!(ask[2], sudo.id().to_string());
    let words: Vec<String> = ask[3..].iter().map(|word| unhex(word)).collect();
    assert_eq!(
        words,
        [
            route.dir.path().display().to_string(),
            "-A".to_owned(),
            "-k".to_owned(),
            "true".to_owned(),
            "x".to_owned()
        ]
    );
    let helper = route.helper().display().to_string();
    route.decide(&format!(
        "go 1 {} {} {}",
        hex(&helper),
        hex("true"),
        hex("x")
    ));
    sudo.wait_with_output().unwrap()
}

#[test]
fn a_root_part_the_daemon_lets_go_exits_with_the_helpers_code() {
    let mut route = Route::start(false);

    let output = let_go(&mut route);

    assert_eq!(output.status.code(), Some(7), "{}", text(&output.stderr));
    assert_eq!(route.next(), ["ended", "1", "exited", "7"]);
}

#[test]
fn the_helper_gets_the_root_command_with_the_cards_folders() {
    let mut route = Route::start(false);

    let_go(&mut route);
    route.next();

    let request: serde_json::Value = serde_json::from_str(&route.file("request")).unwrap();
    let argv = request["argv"].as_array().unwrap();
    let program = argv[0].as_str().unwrap();
    assert!(
        program.starts_with("/usr/") && program.ends_with("/true"),
        "{program}"
    );
    assert_eq!(argv[1], "x");
    assert_eq!(request["run"], 42);
    assert_eq!(request["ask"], 1);
    assert_eq!(request["cwd"], route.dir.path().display().to_string());
    assert_eq!(request["writable"], serde_json::json!(["/srv/site"]));
    assert_eq!(
        request["hidden"],
        serde_json::json!(["/home/o/.config/secret"])
    );
    assert_eq!(request["term"], "xterm-256color");
}

#[test]
fn sudo_asks_through_the_askpass_with_a_prompt_naming_the_root_part() {
    let mut route = Route::start(false);

    let output = let_go(&mut route);

    let log = route.file("sudo.log");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(&lines[..3], ["-A", "-k", "-p"], "{log}");
    assert!(lines[3].contains("root part of run 42 ("), "{log}");
    assert!(lines[3].ends_with("/true x): password for %p: "), "{log}");
    assert_eq!(
        &lines[4..],
        [
            "--",
            route.helper().to_str().unwrap(),
            "root-run",
            "SUDO_ASKPASS=/the/askpass"
        ]
    );
    assert!(
        text(&output.stderr).contains("sudo says hello"),
        "sudo's messages reach the run's terminal"
    );
}

#[test]
fn a_root_part_the_daemon_refuses_runs_no_sudo() {
    let mut route = Route::start(false);
    let sudo = route.sudo(&["make", "install"]);
    route.next();

    route.decide(&format!(
        "refuse 1 {}",
        hex("make install is not on the card")
    ));
    let output = sudo.wait_with_output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("sudo: make install is not on the card"),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(route.file("sudo.log"), "");
}

#[test]
fn a_sudo_that_runs_no_helper_fails_the_runs_sudo_alone() {
    let mut route = Route::start(true);

    let output = let_go(&mut route);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("sudo ran no root part"),
        "{}",
        text(&output.stderr)
    );
    assert_eq!(&route.next()[..3], ["ended", "1", "refused:"]);
    let second = route.sudo(&["true"]);
    assert_eq!(&route.next()[..2], ["ask", "2"]);
    route.decide(&format!("refuse 2 {}", hex("no")));
    assert_eq!(second.wait_with_output().unwrap().status.code(), Some(1));
}

#[test]
fn a_run_without_a_route_is_told_so() {
    let dir = tempfile::tempdir().unwrap();

    let output = Command::new(LAUNCHER)
        .arg("root-route-sudo")
        .arg(dir.path())
        .arg("true")
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains("cannot reach Torlek's root route"),
        "{}",
        text(&output.stderr)
    );
}

#[test]
fn a_daemon_answer_out_of_turn_stops_the_relay() {
    let mut route = Route::start(false);
    let sudo = route.sudo(&["true"]);
    route.next();

    route.decide(&format!("go 9 {} {}", hex("/x"), hex("true")));

    assert!(!route.relay.wait().unwrap().success());
    assert_eq!(sudo.wait_with_output().unwrap().status.code(), Some(1));
}
