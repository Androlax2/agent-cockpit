//! The weekly health check (claude-health-check): a report always, a banner only on problems.

mod common;
use common::sandbox::*;
use std::ffi::CString;
use std::fs;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn health_sandbox() -> Sandbox {
    let sandbox = Sandbox::new();
    sandbox.shim("make", r#"echo "make $*"; [[ $* == *" test" ]] && exit "${SHIM_TESTS_EXIT:-0}"; exit 0"#);
    sandbox.shim("claude-sessions", r#"[[ $1 == doctor ]] && { echo "1 problem(s)"; exit "${SHIM_DOCTOR_EXIT:-0}"; }; exit 0"#);
    let recent = stdout(&Command::new("date").args(["-d", "1 hour ago", "+%a %Y-%m-%d %H:%M:%S %Z"]).output().unwrap());
    let recent = recent.trim();
    sandbox.shim("systemctl", &format!(
        r#"[[ $* == *Result* ]] && echo "${{SHIM_BACKUP:-success}}"; [[ $* == *ExecMainExitTimestamp* ]] && echo "${{SHIM_BACKUP_AT:-{recent}}}"; exit 0"#));
    sandbox.shim("df", r#"echo "Use% Mounted on"; echo " ${SHIM_DISK:-40%} /"; echo " 40% /home""#);
    sandbox
}

fn run_check(sandbox: &Sandbox, extra: &[(&str, &str)]) -> Output {
    let dotfiles = sandbox.root.display().to_string();
    sandbox.command(bin().join("claude-health-check"), &[("CLAUDE_HEALTH_DOTFILES", &dotfiles)]).envs(extra.iter().copied()).output().unwrap()
}

fn report(sandbox: &Sandbox) -> String {
    let path = fs::read_dir(sandbox.state.join("claude-sessions")).unwrap().flatten().map(|entry| entry.path())
        .find(|path| { let name = path.file_name().unwrap().to_string_lossy(); name.starts_with("health-") && name.ends_with(".txt") })
        .unwrap();
    fs::read_to_string(path).unwrap()
}

fn logged(sandbox: &Sandbox) -> Vec<String> {
    sandbox.calls("claude-sessions").into_iter().filter(|call| call.starts_with("claude-sessions log")).collect()
}

/// Kills the fifo's reader and writer even when an assertion fails first.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn a_quiet_week_writes_a_report_and_logs_all_good() {
    let sandbox = health_sandbox();
    assert_eq!(run_check(&sandbox, &[]).status.code(), Some(0));
    assert!(report(&sandbox).contains("== make check (drift, for reading)"));
    assert!(logged(&sandbox).last().unwrap().starts_with("claude-sessions log health INFO weekly check: all good"));
}

#[test]
fn problems_are_named_logged_and_shown_as_a_banner() {
    let sandbox = health_sandbox();
    let fifo = sandbox.runtime.join("osd.fifo");
    let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
    let mut reader = KillOnDrop(Command::new("cat").arg(&fifo).stdout(Stdio::piped()).spawn().unwrap());
    let fifo_writer = fs::OpenOptions::new().write(true).open(&fifo).unwrap();
    let mut holder = KillOnDrop(Command::new("sleep").arg("30").stdout(fifo_writer).spawn().unwrap());
    let output = run_check(&sandbox, &[("SHIM_DOCTOR_EXIT", "1"), ("SHIM_TESTS_EXIT", "2"), ("SHIM_BACKUP", "exit-code"), ("SHIM_DISK", "95%")]);
    assert_eq!(output.status.code(), Some(1));
    let line = logged(&sandbox).last().unwrap().clone();
    for problem in ["doctor: 1 problem(s)", "tests failing", "backup: exit-code", "/ at 95%"] {
        assert!(line.contains(problem), "{problem:?} not in {line:?}");
    }
    std::thread::sleep(Duration::from_millis(300));
    holder.0.kill().unwrap();
    holder.0.wait().unwrap();
    let mut reader_stdout = reader.0.stdout.take().unwrap();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut received = String::new();
        reader_stdout.read_to_string(&mut received).unwrap();
        sender.send(received).unwrap();
    });
    let received = receiver.recv_timeout(Duration::from_secs(5)).expect("the fifo reader did not finish");
    let notice = received.lines().next().unwrap();
    let banner: serde_json::Value = serde_json::from_str(notice.strip_prefix("notice ").expect("not a notice")).unwrap();
    assert_eq!(banner["title"], "Weekly check: 4 problem(s)");
}

#[test]
fn an_old_backup_is_a_problem() {
    let sandbox = health_sandbox();
    run_check(&sandbox, &[("SHIM_BACKUP_AT", "Mon 2026-09-01 10:00:00 CEST")]);
    assert!(logged(&sandbox).last().unwrap().contains("backup older than two days"));
}
