//! craft-open (config/bin/.local/bin/craft-open): FigCraft for the current project, started with
//! its control port and its files limited to the project, never replacing one that is open.

mod common;
use common::sandbox::*;

use std::fs;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, Output};

struct Craft {
    sandbox: Sandbox,
    project: PathBuf,
    /// Stands in for FigCraft's control port: it answers as soon as the test starts.
    app: TcpListener,
}

impl Craft {
    fn new() -> Craft {
        let sandbox = Sandbox::new();
        let project = sandbox.home.join("IdeaProjects/opusline");
        fs::create_dir_all(&project).unwrap();
        sandbox.shim("setsid", "exit 0");
        sandbox.shim("pgrep", r#"[[ -n ${SHIM_FIGCRAFT_PID:-} ]] && echo "$SHIM_FIGCRAFT_PID"; [[ -n ${SHIM_FIGCRAFT_PID:-} ]]"#);
        Craft { sandbox, project, app: TcpListener::bind("127.0.0.1:0").unwrap() }
    }

    fn run(&self, arguments: &[&str], extra: &[(&str, &str)]) -> Output {
        let port = self.app.local_addr().unwrap().port().to_string();
        self.sandbox.command(bin().join("craft-open"), &[("CRAFT_OPEN_PORT", &port), ("CRAFT_OPEN_WAIT_S", "1")])
            .envs(extra.iter().copied()).args(arguments).current_dir(&self.project).output().unwrap()
    }

    fn root(&self) -> String {
        fs::canonicalize(&self.project).unwrap().display().to_string()
    }
}

/// A process named like FigCraft, with the given command line.
fn fake_figcraft(command_line: &str) -> Child {
    Command::new("bash").args(["-c", &format!("exec -a '{command_line}' sleep 30")]).spawn().unwrap()
}

#[test]
fn figcraft_starts_with_its_files_limited_to_the_project() {
    let craft = Craft::new();
    let output = craft.run(&["figcraft"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let started = craft.sandbox.calls("setsid").pop().expect("not started");
    let port = craft.app.local_addr().unwrap().port();
    assert_eq!(started, format!("setsid -f figcraft --control {port} --automation-read-root {0} --automation-write-root {0}", craft.root()));
}

#[test]
fn a_figcraft_open_for_another_folder_is_left_alone() {
    let craft = Craft::new();
    let mut open = fake_figcraft("figcraft --control 1 --automation-write-root /home/elsewhere");
    let output = craft.run(&["figcraft"], &[("SHIM_FIGCRAFT_PID", &open.id().to_string())]);
    let _ = open.kill();
    assert_eq!(output.status.code(), Some(3));
    assert!(craft.sandbox.calls("setsid").is_empty(), "started a second FigCraft");
}

#[test]
fn a_figcraft_already_serving_the_project_is_reused() {
    let craft = Craft::new();
    let port = craft.app.local_addr().unwrap().port();
    let mut open = fake_figcraft(&format!("figcraft --control {port} --automation-read-root {0} --automation-write-root {0}", craft.root()));
    let output = craft.run(&["figcraft"], &[("SHIM_FIGCRAFT_PID", &open.id().to_string())]);
    let _ = open.kill();
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    assert!(stdout(&output).contains("already serves"));
}

#[test]
fn with_a_session_it_schedules_a_restart_and_a_message_to_carry_on() {
    let craft = Craft::new();
    craft.run(&["figcraft", "--session", SESSION_A], &[]);
    let state = craft.sandbox.state.join("claude-sessions");
    assert!(state.join("restart-when-idle").join(SESSION_A).exists());
    let queued: Vec<String> = fs::read_dir(state.join("prompt-queue").join(SESSION_A)).unwrap().flatten()
        .map(|entry| fs::read_to_string(entry.path()).unwrap()).collect();
    assert!(queued[0].contains("FigCraft is connected"), "{queued:?}");
}

#[test]
fn an_unknown_app_or_session_is_refused() {
    let craft = Craft::new();
    assert_eq!(craft.run(&["photoshop"], &[]).status.code(), Some(2));
    assert_eq!(craft.run(&["figcraft", "--session", "../x"], &[]).status.code(), Some(2));
}
