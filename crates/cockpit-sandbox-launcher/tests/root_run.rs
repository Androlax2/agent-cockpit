//! `torlek-sandbox root-run` (#1387): the root part of an approved run, in a system unit with
//! the floor of limits no request lowers.
//!
//! The ignored tests run the helper as root with `sudo -n` and start real system units, so
//! they need passwordless sudo and a system manager: CI's `greenfield-sandbox` job has both
//! and runs them. The test stands in for the daemon: it sends the run's descriptors and the
//! request on the helper's standard input, a Unix socket.
#![cfg(target_os = "linux")]

use std::io::{IoSlice, Read};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use cockpit_sandbox::{ROOT_RUN_DESCRIPTORS_BYTE, RootRunRequest};
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_torlek-sandbox");

/// What one root part printed and the helper answered.
struct Answered {
    stdout: String,
    stderr: String,
    answers: Vec<String>,
    helper: std::process::ExitStatus,
}

impl Answered {
    fn printed(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    fn last_answer(&self) -> &str {
        self.answers.last().map_or("", String::as_str)
    }
}

fn shell(script: &str) -> Vec<String> {
    ["/bin/sh", "-c", script].map(str::to_owned).into()
}

fn request(argv: Vec<String>, writable: &[&Path]) -> RootRunRequest {
    RootRunRequest {
        run: 1,
        ask: 1,
        argv,
        cwd: PathBuf::from("/"),
        writable: writable.iter().map(|path| path.to_path_buf()).collect(),
        hidden: Vec::new(),
        term: None,
    }
}

/// The helper as the daemon starts it: through sudo, in a session of its own, so sudo has no
/// terminal to run it in.
fn helper(daemon_side: UnixStream) -> std::process::Child {
    Command::new("setsid")
        .args(["--wait", "sudo", "-n", "--", LAUNCHER, "root-run"])
        .stdin(Stdio::from(std::os::fd::OwnedFd::from(daemon_side)))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("setsid and sudo are on PATH")
}

fn send(daemon: &UnixStream, request: &RootRunRequest, descriptors: [BorrowedFd<'_>; 3]) {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(3))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    assert!(control.push(SendAncillaryMessage::ScmRights(&descriptors)));
    sendmsg(
        daemon,
        &[IoSlice::new(&[ROOT_RUN_DESCRIPTORS_BYTE])],
        &mut control,
        SendFlags::empty(),
    )
    .unwrap();
    let mut line = serde_json::to_vec(request).unwrap();
    line.push(b'\n');
    std::io::Write::write_all(&mut &*daemon, &line).unwrap();
}

/// Runs `request` to its end and collects what it printed and the helper's answers.
fn root_run(request: &RootRunRequest) -> Answered {
    let (daemon, daemon_side) = UnixStream::pair().unwrap();
    let mut child = helper(daemon_side);
    let input = std::fs::File::open("/dev/null").unwrap();
    let (mut stdout, stdout_writer) = std::io::pipe().unwrap();
    let (mut stderr, stderr_writer) = std::io::pipe().unwrap();
    send(
        &daemon,
        request,
        [input.as_fd(), stdout_writer.as_fd(), stderr_writer.as_fd()],
    );
    drop((input, stdout_writer, stderr_writer));
    let mut answers = String::new();
    (&daemon).read_to_string(&mut answers).unwrap();
    let mut printed_out = String::new();
    stdout.read_to_string(&mut printed_out).unwrap();
    let mut printed_err = String::new();
    stderr.read_to_string(&mut printed_err).unwrap();
    let helper = child.wait().unwrap();
    let mut sudo_said = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut sudo_said)
        .unwrap();
    Answered {
        stdout: printed_out,
        stderr: format!("{printed_err}{sudo_said}"),
        answers: answers.lines().map(str::to_owned).collect(),
        helper,
    }
}

fn fixture() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

fn launcher_as_user(sudo_uid: Option<&str>) -> Output {
    let mut command = Command::new(LAUNCHER);
    command.arg("root-run").env_remove("SUDO_UID");
    if let Some(uid) = sudo_uid {
        command.env("SUDO_UID", uid);
    }
    command.stdin(Stdio::null()).output().unwrap()
}

#[test]
fn root_run_without_sudo_refuses_before_reading_anything() {
    let output = launcher_as_user(None);
    assert_eq!(output.status.code(), Some(125));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("through sudo"), "{stderr}");
}

#[test]
fn root_run_that_is_not_root_refuses_before_reading_anything() {
    let output = launcher_as_user(Some("1000"));
    assert_eq!(output.status.code(), Some(125));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("runs as root"), "{stderr}");
}

#[test]
#[ignore = "needs passwordless sudo and the system manager: CI's greenfield-sandbox job"]
fn a_root_part_runs_as_root_and_exits_with_its_commands_code() {
    let answered = root_run(&request(shell("id -u; exit 7"), &[]));
    assert_eq!(answered.stdout.trim(), "0", "{}", answered.printed());
    assert!(
        answered
            .answers
            .first()
            .is_some_and(|line| line.starts_with("started torlek-root-run-")),
        "{:?}",
        answered.answers
    );
    assert_eq!(answered.last_answer(), "exited 7");
    assert_eq!(answered.helper.code(), Some(7));
}

#[test]
#[ignore = "needs passwordless sudo and the system manager: CI's greenfield-sandbox job"]
fn a_root_part_writes_only_the_folders_it_is_given() {
    let allowed = fixture();
    let other = fixture();
    let script = format!(
        "touch {}/written; touch {}/not-written",
        allowed.path().display(),
        other.path().display()
    );
    let answered = root_run(&request(shell(&script), &[allowed.path()]));
    assert!(
        allowed.path().join("written").exists(),
        "{}",
        answered.printed()
    );
    assert!(!other.path().join("not-written").exists());
    assert!(
        answered.printed().contains("Read-only file system"),
        "{}",
        answered.printed()
    );
}

#[test]
#[ignore = "needs passwordless sudo and the system manager: CI's greenfield-sandbox job"]
fn a_root_part_cannot_change_sudoers_or_leave_a_setuid_program() {
    let folder = fixture();
    let script = format!(
        "touch /etc/sudoers || echo no-sudoers; cp /bin/sh {0}/sh && chmod u+s {0}/sh || echo no-setuid",
        folder.path().display()
    );
    let answered = root_run(&request(shell(&script), &[folder.path()]));
    assert!(
        answered.stdout.contains("no-sudoers"),
        "{}",
        answered.printed()
    );
    assert!(
        answered.stdout.contains("no-setuid"),
        "{}",
        answered.printed()
    );
}

#[test]
#[ignore = "needs passwordless sudo and the system manager: CI's greenfield-sandbox job"]
fn a_root_part_that_asks_to_write_all_of_etc_runs_nothing() {
    let answered = root_run(&request(shell("echo ran"), &[Path::new("/etc")]));
    assert!(!answered.stdout.contains("ran"), "{}", answered.printed());
    assert!(
        answered.last_answer().contains("would write all of /etc"),
        "{:?}",
        answered.answers
    );
}

#[test]
#[ignore = "needs passwordless sudo and the system manager: CI's greenfield-sandbox job"]
fn a_root_part_cannot_read_roots_home_or_the_password_hashes() {
    // Root keeps CAP_DAC_OVERRIDE, so systemd's mode-0000 stand-ins open: hidden reads as empty.
    let answered = root_run(&request(
        shell(
            "echo shadow=$(cat /etc/shadow 2>/dev/null | wc -c); echo root=$(ls -A /root 2>/dev/null | wc -l)",
        ),
        &[],
    ));
    assert!(
        answered.stdout.contains("shadow=0\n"),
        "{}",
        answered.printed()
    );
    assert!(
        answered.stdout.contains("root=0\n"),
        "{}",
        answered.printed()
    );
}

#[test]
#[ignore = "needs passwordless sudo and the system manager: CI's greenfield-sandbox job"]
fn a_root_part_reaches_no_bus_no_network_and_no_process_outside() {
    let answered = root_run(&request(
        shell("echo run=$(ls -A /run | wc -l); echo net=$(ls /sys/class/net); echo pid=$$"),
        &[],
    ));
    let printed = answered.printed();
    assert!(answered.stdout.contains("run=0"), "{printed}");
    assert!(answered.stdout.contains("net=lo"), "{printed}");
    assert!(answered.stdout.contains("pid=1"), "{printed}");
}

#[test]
#[ignore = "needs passwordless sudo and the system manager: CI's greenfield-sandbox job"]
fn a_root_part_cannot_mount_or_keep_the_capabilities_that_undo_its_limits() {
    let answered = root_run(&request(
        shell("grep CapBnd /proc/self/status; mount -t tmpfs none /mnt || echo no-mount"),
        &[],
    ));
    let printed = answered.printed();
    assert!(answered.stdout.contains("no-mount"), "{printed}");
    let bounding = answered
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("CapBnd:"))
        .map(|hex| u64::from_str_radix(hex.trim(), 16).unwrap())
        .unwrap_or_else(|| panic!("no CapBnd line: {printed}"));
    const CAP_DAC_READ_SEARCH: u32 = 2;
    const CAP_SYS_PTRACE: u32 = 19;
    const CAP_SYS_ADMIN: u32 = 21;
    for capability in [CAP_DAC_READ_SEARCH, CAP_SYS_PTRACE, CAP_SYS_ADMIN] {
        assert_eq!(
            bounding & (1 << capability),
            0,
            "capability {capability}: {printed}"
        );
    }
}

#[test]
#[ignore = "needs passwordless sudo and the system manager: CI's greenfield-sandbox job"]
fn a_request_the_floor_refuses_runs_nothing() {
    let answered = root_run(&request(shell("echo ran"), &[Path::new("/boot")]));
    assert!(!answered.stdout.contains("ran"), "{}", answered.printed());
    assert!(
        answered.last_answer().starts_with("refused: ") && answered.last_answer().contains("/boot"),
        "{:?}",
        answered.answers
    );
    assert_eq!(answered.helper.code(), Some(125));
}

#[test]
#[ignore = "needs passwordless sudo and the system manager: CI's greenfield-sandbox job"]
fn a_daemon_that_goes_stops_its_root_part() {
    let (daemon, daemon_side) = UnixStream::pair().unwrap();
    let mut child = helper(daemon_side);
    let input = std::fs::File::open("/dev/null").unwrap();
    send(
        &daemon,
        &request(shell("sleep 300"), &[]),
        [input.as_fd(), input.as_fd(), input.as_fd()],
    );
    let mut started = String::new();
    let mut byte = [0_u8; 1];
    while (&daemon).read(&mut byte).unwrap() == 1 && byte[0] != b'\n' {
        started.push(char::from(byte[0]));
    }
    let unit = started
        .strip_prefix("started ")
        .unwrap_or_else(|| panic!("not started: {started:?}"))
        .to_owned();
    drop(daemon);
    let deadline = Instant::now() + Duration::from_secs(30);
    while child.try_wait().unwrap().is_none() {
        assert!(
            Instant::now() < deadline,
            "the helper still waits for {unit}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let state = Command::new("systemctl")
        .args(["show", "--property=ActiveState", "--value", &unit])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&state.stdout);
    assert!(
        matches!(state.trim(), "inactive" | "failed"),
        "{unit}: {state}"
    );
}

fn hex(word: &str) -> String {
    word.bytes().map(|byte| format!("{byte:02x}")).collect()
}

/// A confined run's `sudo` (#1387, part 4c), end to end: `root-route-sudo` hands its command
/// to `run-root-route`, which this test answers as the daemon would, and the relay runs the
/// helper through the host's sudo.
#[test]
#[ignore = "needs passwordless sudo and the system manager: CI's greenfield-sandbox job"]
fn a_runs_sudo_reaches_root_through_the_root_route() {
    use std::io::{BufRead, BufReader, Write};

    let folder = fixture();
    // In a session of its own, as the daemon's children are: sudo has no terminal.
    let mut relay = Command::new("setsid")
        .arg(LAUNCHER)
        .arg("run-root-route")
        .arg(folder.path())
        .args(["/usr/bin/sudo", "/no/askpass/needed", "1", ""])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("setsid is on PATH");
    let mut decisions = relay.stdin.take().unwrap();
    let mut asks = BufReader::new(relay.stdout.take().unwrap());
    let mut line = String::new();
    asks.read_line(&mut line).unwrap();
    assert_eq!(line, "ready\n");
    let script = "id -u; exit 7";
    let sudo = Command::new(LAUNCHER)
        .arg("root-route-sudo")
        .arg(folder.path())
        .args(["sh", "-c", script])
        .current_dir("/")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    line.clear();
    asks.read_line(&mut line).unwrap();
    assert!(line.starts_with("ask 1 "), "{line}");

    writeln!(
        decisions,
        "go 1 {} {} {} {}",
        hex(LAUNCHER),
        hex("sh"),
        hex("-c"),
        hex(script)
    )
    .unwrap();
    let output = sudo.wait_with_output().unwrap();

    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "0",
        "{printed}"
    );
    assert_eq!(output.status.code(), Some(7), "{printed}");
    line.clear();
    asks.read_line(&mut line).unwrap();
    assert_eq!(line, "ended 1 exited 7\n");
    let _ = relay.kill();
    let _ = relay.wait();
}
