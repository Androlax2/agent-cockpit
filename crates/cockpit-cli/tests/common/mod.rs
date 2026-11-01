//! A clean home with the real `cockpit-daemon` running in it and a fake `git` first on its
//! `PATH`. The fake logs every call, then runs the machine's Git, with GitHub URLs redirected
//! to a local bare repository so pushes and pull-request fetches stay on this machine.
#![allow(
    dead_code,
    reason = "each test file uses a different part of the fixture"
)]

use std::io::{BufRead, BufReader, Read};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Long enough never to fire in a passing test, short enough to fail a hung one quickly.
const GENEROUS: Duration = Duration::from_secs(60);
/// The origin every fixture project pushes to; the fake redirects it to [`TestHome::github`].
pub const GITHUB_URL: &str = "https://github.com/example/app.git";

/// The fake `git`. It appends its arguments to `$FAKE_GIT_LOG`, exits 128 when one argument
/// equals `$FAKE_GIT_FAIL`, and runs `$FAKE_GIT_REAL`. For `push` and `fetch` only, it inserts
/// two settings after the caller's leading `-c` pairs, where they win over the caller's own,
/// so the transfer goes to the local bare repository while every check reads the configured
/// GitHub URL.
const FAKE_GIT: &str = r#"#!/bin/sh
printf '%s\n' "$*" >> "$FAKE_GIT_LOG"
for argument in "$@"; do
    if [ -n "$FAKE_GIT_FAIL" ] && [ "$argument" = "$FAKE_GIT_FAIL" ]; then
        echo "fake git: $FAKE_GIT_FAIL fails" >&2
        exit 128
    fi
done
remaining=$#
inserted=no
while [ "$remaining" -gt 0 ]; do
    argument=$1
    shift
    remaining=$((remaining - 1))
    if [ "$inserted" = no ] && [ "$argument" = -c ] && [ "$remaining" -gt 0 ]; then
        set -- "$@" -c "$1"
        shift
        remaining=$((remaining - 1))
        continue
    fi
    if [ "$inserted" = no ]; then
        case "$argument" in
            push | fetch)
                set -- "$@" -c "url.file://$FAKE_GIT_GITHUB_DIR.insteadOf=$FAKE_GIT_GITHUB_URL" \
                    -c protocol.file.allow=always
                ;;
        esac
        inserted=yes
    fi
    set -- "$@" "$argument"
done
exec "$FAKE_GIT_REAL" "$@"
"#;

pub struct TestHome {
    pub root: tempfile::TempDir,
    /// Arguments that make the fake `git` fail, set before starting the daemon.
    pub failing_git_argument: Option<String>,
    /// Extra variables for the daemon only.
    pub daemon_env: Vec<(String, String)>,
}

impl TestHome {
    pub fn new() -> TestHome {
        let root = tempfile::tempdir().expect("a temporary home");
        let home = TestHome {
            root,
            failing_git_argument: None,
            daemon_env: Vec::new(),
        };
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(home.home().join("run"))
            .unwrap();
        std::fs::create_dir_all(home.home().join("bin")).unwrap();
        let fake = home.home().join("bin/git");
        std::fs::write(&fake, FAKE_GIT).unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        real_git(
            &home.home(),
            &["init", "--quiet", "--bare", home.github().to_str().unwrap()],
        );
        home
    }

    /// The canonical home: macOS's temporary directory is behind a symbolic link.
    pub fn home(&self) -> PathBuf {
        self.root.path().canonicalize().unwrap()
    }

    /// The bare repository standing in for GitHub.
    pub fn github(&self) -> PathBuf {
        self.home().join("github.git")
    }

    pub fn git_log(&self) -> Vec<String> {
        std::fs::read_to_string(self.home().join("git.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// `<home>/projects/<name>`: a repository with one commit on `main`, pushed to the fake
    /// GitHub, whose origin is [`GITHUB_URL`].
    pub fn project(&self, name: &str) -> PathBuf {
        let root = self.home().join("projects").join(name);
        std::fs::create_dir_all(&root).unwrap();
        real_git(&root, &["init", "--quiet", "--initial-branch=main"]);
        std::fs::write(root.join("README"), "fixture\n").unwrap();
        commit_all(&root, "initial");
        real_git(
            &root,
            &["push", "--quiet", self.github().to_str().unwrap(), "main"],
        );
        real_git(&root, &["remote", "add", "origin", GITHUB_URL]);
        real_git(
            &root,
            &[
                "fetch",
                "--quiet",
                self.github().to_str().unwrap(),
                "+main:refs/remotes/origin/main",
            ],
        );
        root
    }

    pub fn trust(&self, root: &Path) {
        let config = self.home().join("config/cockpit");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            format!("[projects]\ntrusted = [{:?}]\n", root.display().to_string()),
        )
        .unwrap();
    }

    fn base_command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .env("HOME", self.home())
            .env("XDG_STATE_HOME", self.home().join("state"))
            .env("XDG_CONFIG_HOME", self.home().join("config"))
            .env("XDG_RUNTIME_DIR", self.home().join("run"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    /// Starts the daemon with the fake `git` first on its `PATH` and waits for `ready`.
    pub fn start(&self) -> RunningDaemon {
        let mut command = self.base_command(&daemon_binary());
        let path = format!(
            "{}:{}",
            self.home().join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        command
            .env("PATH", path)
            .env("FAKE_GIT_LOG", self.home().join("git.log"))
            .env("FAKE_GIT_REAL", real_git_program())
            .env("FAKE_GIT_GITHUB_DIR", self.github())
            .env("FAKE_GIT_GITHUB_URL", GITHUB_URL);
        if let Some(argument) = &self.failing_git_argument {
            command.env("FAKE_GIT_FAIL", argument);
        }
        for (variable, value) in &self.daemon_env {
            command.env(variable, value);
        }
        let mut child = command.spawn().expect("the daemon binary runs");
        let stdout = child.stdout.take().expect("stdout is piped");
        let (lines, ready) = mpsc::channel();
        std::thread::spawn(move || {
            let mut first = String::new();
            let _ = BufReader::new(stdout).read_line(&mut first);
            let _ = lines.send(first);
        });
        let line = ready.recv_timeout(GENEROUS).unwrap_or_default();
        let mut daemon = RunningDaemon { child };
        if !line.starts_with("ready ") {
            let mut stderr = String::new();
            let _ = daemon.child.kill();
            let _ = daemon
                .child
                .stderr
                .take()
                .map(|mut pipe| pipe.read_to_string(&mut stderr));
            panic!("the daemon did not report ready: stdout {line:?}, stderr {stderr:?}");
        }
        daemon
    }

    /// Runs `cockpit <arguments>` and waits for it.
    pub fn cockpit(&self, arguments: &[&str]) -> Output {
        self.base_command(Path::new(env!("CARGO_BIN_EXE_cockpit")))
            .args(arguments)
            .output()
            .expect("the cockpit binary runs")
    }

    /// Runs `cockpit <arguments>` from `dir` and waits for it.
    pub fn cockpit_in(&self, dir: &Path, arguments: &[&str]) -> Output {
        self.base_command(Path::new(env!("CARGO_BIN_EXE_cockpit")))
            .current_dir(dir)
            .args(arguments)
            .output()
            .expect("the cockpit binary runs")
    }

    /// Runs `cockpit <arguments>`, failing the test unless it succeeds; returns stdout.
    pub fn cockpit_ok(&self, arguments: &[&str]) -> String {
        let output = self.cockpit(arguments);
        assert!(
            output.status.success(),
            "cockpit {arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Runs `cockpit <arguments>`, failing the test if it succeeds; returns stderr.
    pub fn cockpit_err(&self, arguments: &[&str]) -> String {
        let output = self.cockpit(arguments);
        assert!(
            !output.status.success(),
            "cockpit {arguments:?} succeeded: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        String::from_utf8(output.stderr).unwrap()
    }
}

pub struct RunningDaemon {
    child: Child,
}

impl RunningDaemon {
    /// `kill -9`: no cleanup runs, as after a crash.
    pub fn kill(mut self) {
        self.child.kill().expect("the daemon is killable");
        self.child.wait().expect("the daemon is reaped");
    }
}

impl Drop for RunningDaemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The `cockpit-daemon` binary of the same profile as the `cockpit` under test. The CLI may
/// not depend on the daemon crate (rule `nothing -> daemon`), so Cargo has not built it for
/// this test: build it here, which is a no-op once it is up to date.
fn daemon_binary() -> PathBuf {
    let cli = Path::new(env!("CARGO_BIN_EXE_cockpit"));
    let profile_dir = cli.parent().expect("the binary is in a profile directory");
    let profile = match profile_dir.file_name().and_then(|name| name.to_str()) {
        Some("debug") => "dev",
        Some(name) => name,
        None => panic!("unexpected binary path {}", cli.display()),
    };
    let built = Command::new(env!("CARGO"))
        .args(["build", "--locked", "--quiet", "--profile", profile])
        .args(["--package", "cockpit-daemon", "--bin", "cockpit-daemon"])
        .arg("--manifest-path")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml"))
        .output()
        .expect("cargo runs");
    assert!(
        built.status.success(),
        "building cockpit-daemon failed: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    profile_dir.join("cockpit-daemon")
}

/// The machine's Git, found on the test's own `PATH`.
fn real_git_program() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH is set");
    std::env::split_paths(&path)
        .map(|dir| dir.join("git"))
        .find(|candidate| candidate.is_file())
        .expect("git is installed")
}

/// Runs the machine's Git with a fixed identity, failing the test when it fails.
pub fn real_git(dir: &Path, arguments: &[&str]) -> String {
    let output = Command::new(real_git_program())
        .current_dir(dir)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.com",
        ])
        .args(["-c", "protocol.file.allow=always"])
        .args(arguments)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {arguments:?} in {}: {}",
        dir.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

pub fn commit_all(dir: &Path, message: &str) {
    real_git(dir, &["add", "--all"]);
    real_git(dir, &["commit", "--quiet", "-m", message]);
}
