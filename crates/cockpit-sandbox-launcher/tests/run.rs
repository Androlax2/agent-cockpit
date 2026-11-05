//! `torlek-sandbox run SPEC` with the real bubblewrap on this host: what the mount view
//! leaves an agent in its checkout. Needs `bwrap` on `PATH`, as CI installs it.
#![cfg(target_os = "linux")]

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use cockpit_sandbox::{
    LaunchSpec, NeedsSource, Protected, SPEC_FILE, SPEC_VERSION, Subject, WhenAbsent,
};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_torlek-sandbox");

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    /// A checkout with a Git folder, in Cargo's temporary folder for tests.
    fn new() -> Fixture {
        let root = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let fixture = Fixture { root };
        for folder in ["checkout/.git/hooks", "home", "home-source", "mounts"] {
            std::fs::create_dir_all(fixture.path(folder)).unwrap();
        }
        std::fs::write(fixture.path("checkout/.git/config"), "[core]\n").unwrap();
        fixture
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.path().canonicalize().unwrap().join(relative)
    }

    fn spec(&self, arguments: Vec<String>) -> LaunchSpec {
        let checkout = self.path("checkout");
        let protected = [
            (".git/hooks", WhenAbsent::EmptyFolder),
            (".git/config", WhenAbsent::EmptyFolder),
            (".torlek", WhenAbsent::EmptyFolder),
            (".vscode/tasks.json", WhenAbsent::Unprotected),
        ]
        .into_iter()
        .map(|(path, when_absent)| Protected {
            path: checkout.join(path),
            when_absent,
        })
        .collect();
        LaunchSpec {
            version: SPEC_VERSION,
            subject: Subject::Session {
                session: "s1".to_owned(),
            },
            bubblewrap: bubblewrap(),
            landlock_abi: probed_landlock_abi(),
            program: PathBuf::from("/bin/sh"),
            arguments,
            working_dir: checkout.clone(),
            home: self.path("home"),
            home_source: self.path("home-source"),
            read_only: Vec::new(),
            read_only_declared: Vec::new(),
            sockets: Vec::new(),
            read_write: vec![checkout.clone()],
            folders: Vec::new(),
            protected,
            hidden: vec![checkout.join(".env")],
            needs_source: NeedsSource::Floor,
            forwards: Vec::new(),
        }
    }

    /// What `sh -c line` printed in the sandbox, ending `exit=<code>`.
    fn run(&self, line: &str) -> String {
        self.run_with(vec!["-c".to_owned(), line.to_owned()])
    }

    fn run_with(&self, arguments: Vec<String>) -> String {
        let spec_path = self.write_spec(arguments);
        let output = Command::new(LAUNCHER)
            .arg("run")
            .arg(&spec_path)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        format!(
            "{}{}exit={}\n",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
            output.status.code().unwrap_or(-1)
        )
    }
}

impl Fixture {
    fn write_spec(&self, arguments: Vec<String>) -> PathBuf {
        let spec_path = self.path("mounts").join(SPEC_FILE);
        std::fs::write(
            &spec_path,
            serde_json::to_vec(&self.spec(arguments)).unwrap(),
        )
        .unwrap();
        spec_path
    }

    /// What `sh -c line` printed in the sandbox with a terminal for its standard streams, as
    /// in a tmux pane: run through util-linux's `script`.
    fn run_on_a_terminal(&self, line: &str) -> String {
        let spec_path = self.write_spec(vec!["-c".to_owned(), line.to_owned()]);
        let output = Command::new("script")
            .args(["--quiet", "--return", "--command"])
            .arg(format!("{LAUNCHER} run {}", spec_path.display()))
            .arg("/dev/null")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("util-linux's `script` runs");
        String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n")
    }
}

fn bubblewrap() -> PathBuf {
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .map(|folder| folder.join("bwrap"))
        .find(|candidate| candidate.is_file())
        .expect("bubblewrap (`bwrap`) on PATH")
}

fn probed_landlock_abi() -> u8 {
    let output = Command::new(LAUNCHER)
        .arg("probe")
        .arg(bubblewrap())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    u8::try_from(report["landlock_abi"].as_u64().unwrap()).unwrap()
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn the_checkout_is_writable() {
    let fixture = Fixture::new();

    let printed = fixture.run("echo written > written && cat written");

    assert_eq!(printed, "written\nexit=0\n");
}

/// A worktree's `.git` names its Git folder; rewritten, it would point the owner's Git at
/// hooks of the agent's.
#[test]
fn a_worktrees_git_file_is_read_only() {
    let fixture = Fixture::new();
    std::fs::remove_dir_all(fixture.path("checkout/.git")).unwrap();
    write(&fixture.path("checkout/.git"), "gitdir: /elsewhere\n");

    let printed = fixture.run("echo 'gitdir: /planted' 2>/dev/null > .git || echo refused");

    assert!(printed.starts_with("refused\n"), "{printed}");
    assert_eq!(
        std::fs::read_to_string(fixture.path("checkout/.git")).unwrap(),
        "gitdir: /elsewhere\n"
    );
}

/// A hidden `.env` that links to a file in the checkout hides that file too.
#[test]
fn a_hidden_link_hides_its_target() {
    let fixture = Fixture::new();
    write(
        &fixture.path("checkout/config/secrets.env"),
        "TOKEN=secret\n",
    );
    symlink("config/secrets.env", fixture.path("checkout/.env")).unwrap();

    let printed = fixture.run("cat .env config/secrets.env; echo read");

    assert!(printed.starts_with("read\n"), "{printed}");
    assert!(!printed.contains("TOKEN"), "{printed}");
}

/// `.vscode/tasks.json` is protected only when present; without it the editor's folder
/// stays an ordinary folder the agent may remove.
#[test]
fn an_absent_editor_file_leaves_its_folder_removable() {
    let fixture = Fixture::new();
    write(&fixture.path("checkout/.vscode/settings.json"), "{}\n");

    let printed = fixture.run("rm -r .vscode && echo removed");

    assert_eq!(printed, "removed\nexit=0\n");
}

/// Each argument keeps the kernel's per-argument limit (128 KiB) it has unsandboxed, however
/// long they are together.
#[test]
fn long_arguments_reach_the_program_each_on_its_own() {
    let fixture = Fixture::new();
    let long = "x".repeat(100 * 1024);

    let printed = fixture.run_with(vec![
        "-c".to_owned(),
        "echo \"${#1} ${#2}\"".to_owned(),
        "sh".to_owned(),
        long.clone(),
        long,
    ]);

    assert_eq!(printed, "102400 102400\nexit=0\n");
}

/// Tools write to `/dev/stdout` and `/dev/stderr`, which name the pane's terminal.
#[test]
fn a_program_on_a_terminal_writes_to_dev_stdout_and_dev_stderr() {
    let fixture = Fixture::new();

    let printed = fixture.run_on_a_terminal("echo out > /dev/stdout && echo err > /dev/stderr");

    assert_eq!(printed, "out\nerr\n");
}
