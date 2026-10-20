//! A scratch home, runtime and state folder, and fake commands on PATH that log how they were
//! called: the scripts under test see only this environment, the host stays untouched.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const SESSION_A: &str = "aaaaaaaa-0000-4000-8000-000000000000";
pub const SESSION_B: &str = "bbbbbbbb-0000-4000-8000-000000000000";
/// An opencode session's id.
pub const OPENCODE_SESSION: &str = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L";
/// The answers folder the launcher names for the sandbox's project (CLAUDE_ANSWERS_KEY).
pub const ANSWERS_KEY: &str = "-home-theo-IdeaProjects-proj-a";

/// The dotfiles checkout these tests belong to.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

/// config/bin/.local/bin: the wrappers and scripts on Theo's PATH.
pub fn bin() -> PathBuf {
    repo().join("config/bin/.local/bin")
}

pub fn waybar_scripts() -> PathBuf {
    repo().join("config/waybar/.config/waybar/scripts")
}

/// SESSION_STATE_HOOK points the hook tests at another copy, such as the one in .handoff/claude
/// before it is installed (config/claude is read-only for Claude inside the sandbox).
pub fn hook() -> PathBuf {
    std::env::var("SESSION_STATE_HOOK").map(PathBuf::from).unwrap_or_else(|_| repo().join("config/claude/.claude/hooks/session-state.sh"))
}

/// An executable file with `content`, written by a short-lived `sh`, never by this process: tests
/// run in parallel threads, and a file this process holds open for writing is inherited by any
/// child another thread forks meanwhile, which makes running it fail with "Text file busy".
pub fn write_executable(path: &Path, content: &str) {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    let mut writer = Command::new("sh").args(["-c", "cat > \"$1\" && chmod 755 \"$1\"", "sh"]).arg(path).stdin(Stdio::piped()).spawn().unwrap();
    writer.stdin.take().unwrap().write_all(content.as_bytes()).unwrap();
    assert!(writer.wait().unwrap().success(), "cannot write {}", path.display());
}

/// A copy of an executable, made by `cp` for the same reason as `write_executable`.
pub fn copy_executable(from: &Path, to: &Path) {
    assert!(Command::new("cp").arg(from).arg(to).status().unwrap().success(), "cannot copy {}", from.display());
}

pub struct Sandbox {
    pub root: PathBuf,
    pub home: PathBuf,
    pub runtime: PathBuf,
    pub state: PathBuf,
    pub shims: PathBuf,
    pub log: PathBuf,
    /// Added to (and overriding) the test process's own environment for every command.
    pub env: HashMap<String, String>,
}

impl Sandbox {
    pub fn new() -> Sandbox {
        let root = std::env::temp_dir().join(format!("claude-sessions-test-{}", cockpit::util::token_hex(6)));
        let (home, runtime, state, shims) = (root.join("home"), root.join("runtime"), root.join("state"), root.join("shims"));
        for path in [&home, &runtime, &state, &shims] {
            fs::create_dir_all(path).unwrap();
        }
        let log = root.join("calls.log");
        fs::write(&log, "").unwrap();
        // The scripts find their siblings through $HOME; link the real ones in.
        let scripts = home.join(".config/waybar/scripts");
        fs::create_dir_all(&scripts).unwrap();
        for entry in fs::read_dir(waybar_scripts()).unwrap().flatten() {
            symlink(entry.path(), scripts.join(entry.file_name())).unwrap();
        }
        let local_bin = home.join(".local/bin");
        fs::create_dir_all(&local_bin).unwrap();
        for name in ["claude-sessions", "claude-sessions-sidebar", "osd", "osd-show", "claude-audit"] {
            symlink(bin().join(name), local_bin.join(name)).unwrap();
        }
        let path = format!("{}:{}:{}", shims.display(), local_bin.display(), std::env::var("PATH").unwrap_or_default());
        let env = HashMap::from([
            ("HOME".to_string(), home.display().to_string()),
            ("XDG_RUNTIME_DIR".into(), runtime.display().to_string()),
            ("XDG_STATE_HOME".into(), state.display().to_string()),
            // Wins over XDG_STATE_HOME in the cockpit's tools, and opencode sessions set it.
            ("CLAUDE_SESSIONS_STATE_HOME".into(), state.display().to_string()),
            ("PATH".into(), path),
            ("SHIM_LOG".into(), log.display().to_string()),
            ("CLAUDE_STATUS_URL".into(), String::new()), // no network: status.claude.com is not fetched
            ("CODEX_STATUS_URL".into(), String::new()),
            ("CLAUDE_ANSWERS_KEY".into(), ANSWERS_KEY.to_string()),
            ("CLAUDE_AUDIT_JOURNAL".into(), "off".to_string()),
        ]);
        Sandbox { root, home, runtime, state, shims, log, env }
    }

    /// A fake command. `body` is bash; $SHIM_LOG receives one line per call by convention.
    pub fn shim(&self, name: &str, body: &str) {
        write_executable(&self.shims.join(name), &format!("#!/usr/bin/env bash\necho \"{name} $*\" >> \"$SHIM_LOG\"\n{body}\n"));
    }

    /// The logged calls of one command, "name arguments" each.
    pub fn calls(&self, name: &str) -> Vec<String> {
        let prefix = format!("{name} ");
        fs::read_to_string(&self.log).unwrap_or_default().lines().filter(|line| line.starts_with(&prefix)).map(str::to_string).collect()
    }

    /// For commands the code runs in the background: their log lines arrive a little later.
    pub fn wait_for_calls(&self, name: &str, count: usize, timeout: Duration) -> Vec<String> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let calls = self.calls(name);
            if calls.len() >= count {
                return calls;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        self.calls(name)
    }

    pub fn reset_log(&self) {
        fs::write(&self.log, "").unwrap();
    }

    /// `program` with the sandbox environment, plus `extra` on top.
    pub fn command(&self, program: impl AsRef<std::ffi::OsStr>, extra: &[(&str, &str)]) -> Command {
        let mut command = Command::new(program);
        command.envs(&self.env).envs(extra.iter().copied()).stdin(Stdio::null());
        command
    }

    /// Runs `program arguments` to the end, output captured.
    pub fn run(&self, program: impl AsRef<std::ffi::OsStr>, arguments: &[&str], extra: &[(&str, &str)]) -> Output {
        self.command(program, extra).args(arguments).output().unwrap()
    }

    /// Like `run`, with `input` on stdin.
    pub fn run_with_input(&self, program: impl AsRef<std::ffi::OsStr>, arguments: &[&str], extra: &[(&str, &str)], input: &str) -> Output {
        use std::io::Write;
        let mut child = self.command(program, extra).args(arguments).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().unwrap();
        if let Err(error) = child.stdin.take().unwrap().write_all(input.as_bytes()) {
            // Commands can reject a request before consuming stdin; keep their diagnostics.
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "cannot write child stdin: {error}");
        }
        child.wait_with_output().unwrap()
    }
}

/// What a fake team daemon received, in order.
pub type TeamRequests = std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>;

impl Sandbox {
    /// A fake team daemon, linked into this sandbox's answers folder as the real one is: answers
    /// `receive` with `delivery` once (then null), everything else "recorded".
    pub fn team_socket(&self, delivery: serde_json::Value) -> TeamRequests {
        use std::io::{Read, Write};
        let source = self.root.join(format!("team-{}.sock", cockpit::util::token_hex(4)));
        let listener = std::os::unix::net::UnixListener::bind(&source).unwrap();
        let answers = self.runtime.join("claude-answers").join(ANSWERS_KEY);
        fs::create_dir_all(&answers).unwrap();
        cockpit::teams::client::publish_socket(&source, &answers).unwrap();
        let requests = TeamRequests::default();
        let recorded = requests.clone();
        std::thread::spawn(move || {
            let mut pending = Some(delivery);
            for mut stream in listener.incoming().flatten() {
                let mut text = String::new();
                let _ = stream.read_to_string(&mut text);
                let Ok(request) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
                let result = if request["action"] == "receive" { pending.take().unwrap_or(serde_json::Value::Null) }
                             else { serde_json::json!({"status": "recorded"}) };
                recorded.lock().unwrap().push(request);
                let _ = stream.write_all(serde_json::json!({"request_id": "x", "result": {"Ok": result}}).to_string().as_bytes());
            }
        });
        requests
    }

    /// Theo's answers to this sandbox's project, "runs" or "guides" (cockpit::approval::answers_dir).
    pub fn answers(&self, kind: &str) -> PathBuf {
        let directory = self.runtime.join("claude-answers").join(ANSWERS_KEY).join(kind);
        fs::create_dir_all(&directory).unwrap();
        directory
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A live process with real argument boundaries for /proc discovery tests. Its pipe stays open
/// so Bash's builtin read waits without leaving a sleep child behind when the test ends.
pub struct WaitingProcess(Child);

impl WaitingProcess {
    pub fn new(name: &str, arguments: &[&str], directory: &Path) -> Self {
        Self::with_environment(name, arguments, directory, &[])
    }

    pub fn with_environment(name: &str, arguments: &[&str], directory: &Path, environment: &[(&str, &str)]) -> Self {
        let child = Command::new("bash").args(["-c", "read -r -t 60", name]).args(arguments)
            .current_dir(directory).envs(environment.iter().copied()).stdin(Stdio::piped()).spawn().unwrap();
        Self(child)
    }

    pub fn id(&self) -> String { self.0.id().to_string() }

    pub fn is_alive(&mut self) -> bool { self.0.try_wait().unwrap().is_none() }
}

impl Drop for WaitingProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Waits for `child` to exit and collects its output; kills it and fails past `timeout`.
pub fn finish(child: Child, timeout: Duration) -> Output {
    let pid = child.id() as libc::pid_t;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || sender.send(child.wait_with_output()));
    match receiver.recv_timeout(timeout) {
        Ok(output) => output.unwrap(),
        Err(_) => {
            unsafe { libc::kill(pid, libc::SIGKILL) };
            panic!("process {pid} still running after {timeout:?}");
        }
    }
}

/// A stand-in for the OSD daemon: `cat` reads its fifo, `sleep` holds it open for writing.
pub struct Listener {
    reader: Option<Child>,
    holder: Child,
}

impl Listener {
    pub fn start(fifo: &Path) -> Listener {
        let path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0, "mkfifo {}", fifo.display());
        let reader = Command::new("cat").arg(fifo).stdout(Stdio::piped()).spawn().unwrap();
        let writer = fs::OpenOptions::new().write(true).open(fifo).unwrap();
        let holder = Command::new("sleep").arg("30").stdout(writer).spawn().unwrap();
        Listener { reader: Some(reader), holder }
    }

    /// Closes the fifo and returns everything written to it.
    pub fn received(&mut self) -> String {
        let _ = self.holder.kill();
        let _ = self.holder.wait();
        stdout(&finish(self.reader.take().unwrap(), Duration::from_secs(5)))
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.holder.kill();
        let _ = self.holder.wait();
        if let Some(mut reader) = self.reader.take() {
            let _ = reader.kill();
            let _ = reader.wait();
        }
    }
}

/// A stand-in for the OSD daemon's report socket: it reads each connection to its end, keeps the
/// lines, and answers `reply` ("ok", or "refused: <reason>") as the daemon does to a request.
pub struct ReportSocket {
    path: PathBuf,
    lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl ReportSocket {
    pub fn start(path: &Path, reply: &str) -> ReportSocket {
        use std::io::{Read, Write};
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(path).unwrap();
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (kept, reply) = (lines.clone(), format!("{reply}\n"));
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut text = String::new();
                let _ = stream.read_to_string(&mut text);
                kept.lock().unwrap().extend(text.lines().map(str::to_string));
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        ReportSocket { path: path.to_path_buf(), lines }
    }

    /// The lines received so far, waiting up to `timeout` for the first one.
    pub fn received(&self, timeout: Duration) -> Vec<String> {
        let deadline = Instant::now() + timeout;
        while self.lines.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        self.lines.lock().unwrap().clone()
    }
}

impl Drop for ReportSocket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}
