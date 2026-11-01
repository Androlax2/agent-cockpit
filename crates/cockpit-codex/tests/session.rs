//! `cockpit-codex-session` as a process (#407): it runs the app-server and the TUI against
//! it, reports a conversation's failed turns through `cockpit`, and stops the app-server
//! when the TUI exits. Codex is a fake script; its app-server is the fake one, bound once
//! the script started.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use cockpit_provider_api::ConversationId;
use common::app_server::{Answer, FakeAppServer};
use serde_json::{Value, json};

const THREAD: &str = "01a1981e-525b-75f2-902e-1e2e6d337eea";
const HELPER_THREAD: &str = "01a1981e-6eb4-70d3-a4a5-f74762d56dd3";
const OTHER_THREAD: &str = "01a1981e-7a10-7c11-b0a2-0d1e2f3a4b5c";

/// The fake `codex`: as `app-server`, it records its words and pid and sleeps; as the TUI,
/// it records its words and exits 3 once the test creates `tui.exit`.
const FAKE_CODEX: &str = r#"if [ "$1" = app-server ]; then
  printf '%s\n' "$@" > "$FAKE_DIR/server.args"
  echo $$ > "$FAKE_DIR/server.pid.partial" && mv "$FAKE_DIR/server.pid.partial" "$FAKE_DIR/server.pid"
  exec sleep 600
fi
printf '%s\n' "$@" > "$FAKE_DIR/tui.args"
while [ ! -e "$FAKE_DIR/tui.exit" ]; do sleep 0.05; done
exit 3"#;

/// A fake `codex` whose app-server ignores `SIGTERM` and runs a child that does too, as a
/// wrapper that does not pass the signal on would leave it.
const FAKE_CODEX_IGNORING_TERM: &str = r#"if [ "$1" = app-server ]; then
  trap '' TERM
  sleep 600 &
  echo $! > "$FAKE_DIR/server-child.pid"
  echo $$ > "$FAKE_DIR/server.pid.partial" && mv "$FAKE_DIR/server.pid.partial" "$FAKE_DIR/server.pid"
  wait
  exit 0
fi
touch "$FAKE_DIR/tui.args"
while [ ! -e "$FAKE_DIR/tui.exit" ]; do sleep 0.05; done
exit 3"#;

const FAKE_COCKPIT: &str = r#"printf '%s\n' "$@" >> "$FAKE_DIR/reports""#;

struct Launcher {
    dir: tempfile::TempDir,
    child: Option<Child>,
}

impl Launcher {
    fn start(codex_body: &str, extra: &[&str]) -> Launcher {
        let dir = tempfile::tempdir().unwrap();
        let codex = common::script(dir.path(), "codex", codex_body);
        let cockpit = common::script(dir.path(), "cockpit", FAKE_COCKPIT);
        let child = Command::new(env!("CARGO_BIN_EXE_cockpit-codex-session"))
            .args(["--cockpit".as_ref(), cockpit.as_os_str()])
            .args(["--codex".as_ref(), codex.as_os_str()])
            .args([
                "--socket".as_ref(),
                dir.path().join("codex.sock").as_os_str(),
            ])
            .args(extra)
            .env("FAKE_DIR", dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Launcher {
            dir,
            child: Some(child),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// The fake app-server on the session's socket, once the fake `codex app-server` runs.
    fn serve(
        &self,
        script: impl Fn(&str, &Value) -> Answer + Send + Sync + 'static,
    ) -> FakeAppServer {
        wait_for_file(&self.path("server.pid"));
        FakeAppServer::start(&self.path("codex.sock"), script)
    }

    fn lines(&self, name: &str) -> Vec<String> {
        let path = self.path(name);
        wait_for_file(&path);
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Lets the fake TUI exit, and the launcher's status and standard error.
    fn finish(&mut self) -> (ExitStatus, String) {
        std::fs::write(self.path("tui.exit"), "").unwrap();
        let output = self.child.take().unwrap().wait_with_output().unwrap();
        (
            output.status,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = std::fs::write(self.path("tui.exit"), "");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn wait_for_file(path: &Path) {
    let ends_at = Instant::now() + Duration::from_secs(20);
    while !path.exists() {
        assert!(
            Instant::now() < ends_at,
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Whether `pid` runs: it exists and is not a zombie.
fn is_running(pid: &str) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
        stat.rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z'))
    })
}

fn wait_until(condition: impl Fn() -> bool) {
    let ends_at = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < ends_at, "the condition never held");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Answers for a session whose conversation is [`THREAD`] and whose other loaded thread is
/// an ephemeral helper; each one's last turn failed on a refused key.
fn failing_threads(method: &str, params: &Value) -> Answer {
    let thread = params["threadId"].as_str().unwrap_or_default();
    match method {
        "thread/read" => Ok(json!({"thread": {
            "id": thread,
            "ephemeral": thread == HELPER_THREAD,
            "parentThreadId": null,
        }})),
        "thread/turns/list" => Ok(json!({"data": [{
            "id": "01a1981d-0cd0-7fb0-82d7-6ebfd0b348d5",
            "status": "failed",
            "error": {"message": "401 Unauthorized", "codexErrorInfo": "unauthorized"},
        }]})),
        _ => Ok(json!({})),
    }
}

fn system_error(thread: &str) -> Value {
    json!({"threadId": thread, "status": {"type": "systemError"}})
}

fn words(arguments: &[std::ffi::OsString]) -> Vec<String> {
    common::words(arguments)
}

#[test]
fn the_app_server_runs_on_the_socket_with_the_hooks_of_the_launch() {
    let launcher = Launcher::start(FAKE_CODEX, &["--config", "hooks.Stop=[]"]);
    let _server = launcher.serve(|_, _| Ok(json!({})));

    assert_eq!(
        launcher.lines("server.args"),
        words(&cockpit_codex::server_arguments(
            &launcher.path("codex.sock"),
            &["hooks.Stop=[]".to_owned()]
        ))
    );
}

#[test]
fn the_tui_resumes_its_thread_against_the_app_server() {
    let launcher = Launcher::start(FAKE_CODEX, &["--resume", THREAD, "--message", "-x hi"]);
    let _server = launcher.serve(|_, _| Ok(json!({})));
    let thread = ConversationId::new(THREAD).unwrap();

    assert_eq!(
        launcher.lines("tui.args"),
        words(&cockpit_codex::tui_arguments(
            &launcher.path("codex.sock"),
            Some(&thread),
            Some("-x hi")
        ))
    );
}

#[test]
fn when_the_tui_exits_the_launcher_stops_the_app_server_and_exits_with_its_status() {
    let mut launcher = Launcher::start(FAKE_CODEX, &[]);
    let _server = launcher.serve(|_, _| Ok(json!({})));
    launcher.lines("tui.args");
    let server_pid = launcher.lines("server.pid")[0].clone();

    let (status, _) = launcher.finish();

    assert_eq!(status.code(), Some(3));
    assert!(
        !Path::new(&format!("/proc/{server_pid}")).exists(),
        "the app-server {server_pid} still runs"
    );
}

#[test]
fn an_app_server_that_ignores_sigterm_is_killed_with_everything_it_started() {
    let mut launcher = Launcher::start(FAKE_CODEX_IGNORING_TERM, &[]);
    let _server = launcher.serve(|_, _| Ok(json!({})));
    launcher.lines("tui.args");
    let child_pid = launcher.lines("server-child.pid")[0].clone();

    let (status, _) = launcher.finish();

    assert_eq!(status.code(), Some(3));
    // Orphaned by the kill, it lingers as a zombie until its new parent reaps it.
    wait_until(|| !is_running(&child_pid));
}

#[test]
fn a_failed_turn_of_the_conversation_is_reported_with_its_outcome() {
    let launcher = Launcher::start(FAKE_CODEX, &[]);
    let server = launcher.serve(failing_threads);
    server.wait_for_connections(1);

    server.notify("thread/status/changed", system_error(THREAD));

    assert_eq!(
        launcher.lines("reports"),
        [
            "agent",
            "report",
            "--state",
            "waiting",
            "--outcome",
            "failed",
            "--outcome-reason",
            "unauthorized",
            "--conversation",
            THREAD
        ]
    );
}

#[test]
fn a_helper_threads_failure_is_not_reported() {
    let launcher = Launcher::start(FAKE_CODEX, &[]);
    let server = launcher.serve(failing_threads);
    server.wait_for_connections(1);

    server.notify("thread/status/changed", system_error(HELPER_THREAD));
    server.notify("thread/status/changed", system_error(THREAD));

    let reports = launcher.lines("reports");
    assert_eq!(reports.last().map(String::as_str), Some(THREAD));
    assert!(!reports.contains(&HELPER_THREAD.to_owned()), "{reports:?}");
}

#[test]
fn a_failed_turn_announced_twice_is_reported_once() {
    let launcher = Launcher::start(FAKE_CODEX, &[]);
    let server = launcher.serve(failing_threads);
    server.wait_for_connections(1);

    server.notify("thread/status/changed", system_error(THREAD));
    server.notify("thread/status/changed", system_error(THREAD));
    server.notify("thread/status/changed", system_error(OTHER_THREAD));
    wait_until(|| launcher.lines("reports").contains(&OTHER_THREAD.to_owned()));

    let reports = launcher.lines("reports");
    assert_eq!(
        reports.iter().filter(|word| *word == THREAD).count(),
        1,
        "{reports:?}"
    );
}

#[test]
fn an_idle_thread_reports_nothing() {
    let launcher = Launcher::start(FAKE_CODEX, &[]);
    let server = launcher.serve(failing_threads);
    server.wait_for_connections(1);

    server.notify(
        "thread/status/changed",
        json!({"threadId": THREAD, "status": {"type": "idle"}}),
    );
    server.notify("thread/status/changed", system_error(THREAD));
    launcher.lines("reports");

    assert_eq!(
        server
            .methods()
            .iter()
            .filter(|method| *method == "thread/read")
            .count(),
        1
    );
}

#[test]
fn an_app_server_that_exits_at_start_fails_the_launch_pointing_at_its_log() {
    let mut launcher = Launcher::start("echo 'no auth' >&2; exit 7", &[]);

    let (status, stderr) = launcher.finish();

    assert_eq!(status.code(), Some(1));
    assert!(stderr.contains("Codex's app-server exited"), "{stderr}");
    assert!(stderr.contains("codex-app-server.log"), "{stderr}");
    let log = std::fs::read_to_string(launcher.path("codex-app-server.log")).unwrap();
    assert!(log.contains("no auth"), "{log}");
}

#[test]
fn a_launch_without_its_socket_is_refused_with_the_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_cockpit-codex-session"))
        .args(["--cockpit", "/bin/true", "--codex", "/bin/true"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("usage: cockpit-codex-session"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
