//! The greywall launcher (config/bin/.local/bin/greywall-agents/claude): what it refuses before
//! any sandbox starts.

mod common;
use common::sandbox::*;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn no_session_starts_in_the_live_dotfiles_checkout() {
    // Security audit C1: a session there could write code the host runs, unreviewed.
    let sandbox = Sandbox::new();
    let live = sandbox.home.join("dotfiles/apps");
    fs::create_dir_all(&live).unwrap();
    sandbox.shim("greywall", "");
    let output = sandbox.command(bin().join("greywall-agents/claude"), &[]).current_dir(&live).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("IdeaProjects/dotfiles"));
    assert!(sandbox.calls("greywall").is_empty());
}

/// A home where the launcher runs to its end: greywall is a fake that logs its arguments, DNS
/// answers at once, there is no keyring and no FigCraft unless a test starts one.
struct Launch {
    sandbox: Sandbox,
    project: PathBuf,
}

impl Launch {
    fn new() -> Launch {
        let sandbox = Sandbox::new();
        let project = sandbox.home.join("IdeaProjects/proj-a");
        fs::create_dir_all(&project).unwrap();
        assert!(Command::new("git").args(["init", "-q"]).arg(&project).status().unwrap().success());
        fs::create_dir_all(sandbox.home.join("IdeaProjects/proj-b")).unwrap();
        fs::create_dir_all(sandbox.home.join("dotfiles")).unwrap();
        for (path, content) in [(".local/share/greyproxy/ca-cert.pem", ""), (".local/share/agent-shims/bin/wl-paste", "#!/bin/sh\n"),
                                (".config/greywall/greywall.json", r#"{"filesystem": {"denyWrite": [".git/hooks"]}}"#)] {
            let file = sandbox.home.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, content).unwrap();
        }
        write_executable(&sandbox.home.join(".local/bin/greywall"), "#!/bin/sh\necho \"greywall $* (api key: ${ANTHROPIC_API_KEY:-none})\" >> \"$SHIM_LOG\"\n");
        sandbox.shim("dig", "echo 127.0.0.1");
        sandbox.shim("secret-tool", r#"[ "$3" = agent-anthropic ] && [ -n "${SHIM_ANTHROPIC_KEY:-}" ] && echo "$SHIM_ANTHROPIC_KEY" || exit 1"#);
        sandbox.shim("pgrep", r#"[ -n "${SHIM_FIGCRAFT_PID:-}" ] && echo "$SHIM_FIGCRAFT_PID""#);
        Launch { sandbox, project }
    }

    fn run(&self, extra: &[(&str, &str)]) -> String {
        let output = self.sandbox.command(bin().join("greywall-agents/claude"), extra).current_dir(&self.project).output().unwrap();
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        self.sandbox.calls("greywall").pop().expect("greywall did not start")
    }

    fn slug(path: &Path) -> String {
        fs::canonicalize(path).unwrap().display().to_string().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
    }

    fn settings(&self) -> serde_json::Value {
        let path = self.sandbox.runtime.join("greywall-agents").join(format!("{}.json", Launch::slug(&self.project)));
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }
}

#[test]
fn other_projects_claude_folders_are_read_only_and_its_own_is_not() {
    // Security audit H2: a session must not plant memory another project loads.
    let launch = Launch::new();
    let started = launch.run(&[]);
    assert!(started.contains("--settings "), "{started}");
    let denied: Vec<String> = launch.settings()["filesystem"]["denyWrite"].as_array().unwrap().iter()
        .map(|path| path.as_str().unwrap().to_string()).collect();
    let projects = launch.sandbox.home.join(".claude/projects");
    let other = projects.join(Launch::slug(&launch.sandbox.home.join("IdeaProjects/proj-b"))).display().to_string();
    let own = projects.join(Launch::slug(&launch.project)).display().to_string();
    assert!(denied.contains(&other), "{denied:?}");
    assert!(!denied.contains(&own), "{denied:?}");
    assert!(denied.contains(&".git/hooks".to_string()), "greywall.json's own rules were lost: {denied:?}");
}

#[test]
fn only_the_projects_own_answers_folder_is_mounted() {
    // Security audit M3: no session reads another project's command output.
    let launch = Launch::new();
    let started = launch.run(&[]);
    let answers = launch.sandbox.runtime.join("claude-answers").join(Launch::slug(&launch.project));
    assert!(started.contains(&format!("--allow-read-path {} ", answers.display())), "{started}");
    assert!(answers.join("runs").is_dir() && answers.join("guides").is_dir());
}

/// A process named like FigCraft, with the given command line.
fn fake_figcraft(command_line: &str) -> std::process::Child {
    Command::new("bash").args(["-c", &format!("exec -a '{command_line}' sleep 30")]).spawn().unwrap()
}

#[test]
fn figcrafts_port_reaches_only_the_project_it_serves() {
    // Security audit M2: whoever reaches the port reads and writes that project's designs.
    let launch = Launch::new();
    let root = fs::canonicalize(&launch.project).unwrap().display().to_string();
    let other = fs::canonicalize(launch.sandbox.home.join("IdeaProjects/proj-b")).unwrap().display().to_string();
    for (served, is_forwarded) in [(&root, true), (&other, false)] {
        let mut figcraft = fake_figcraft(&format!("figcraft --control 7979 --automation-read-root {served} --automation-write-root {served}"));
        let started = launch.run(&[("SHIM_FIGCRAFT_PID", &figcraft.id().to_string())]);
        let _ = figcraft.kill();
        assert_eq!(started.contains("--forward 7979"), is_forwarded, "serving {served}: {started}");
    }
    assert!(!launch.run(&[]).contains("--forward 7979"), "forwarded with no FigCraft open");
}

#[test]
fn a_credits_session_gets_the_api_key_as_a_placeholder() {
    let launch = Launch::new();
    let started = launch.run(&[("CLAUDE_SESSIONS_CREDITS", "1"), ("SHIM_ANTHROPIC_KEY", "sk-ant-test")]);
    assert!(started.contains("--secret ANTHROPIC_API_KEY") && started.contains("(api key: sk-ant-test)"), "{started}");
}

#[test]
fn a_credits_session_without_a_key_does_not_start_on_the_subscription() {
    let launch = Launch::new();
    let output = launch.sandbox.command(bin().join("greywall-agents/claude"), &[("CLAUDE_SESSIONS_CREDITS", "1")]).current_dir(&launch.project).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("agent-anthropic-key"), "{}", stderr(&output));
    assert!(launch.sandbox.calls("greywall").is_empty());
}

#[test]
fn a_subscription_session_gets_no_api_key_even_from_its_environment() {
    let launch = Launch::new();
    let started = launch.run(&[("ANTHROPIC_API_KEY", "sk-ant-stray"), ("SHIM_ANTHROPIC_KEY", "sk-ant-test")]);
    assert!(!started.contains("ANTHROPIC_API_KEY") && started.contains("(api key: none)"), "{started}");
}

#[test]
fn a_parallel_dotfiles_clone_reads_the_live_checkout_and_another_project_does_not() {
    let launch = Launch::new();
    let live = fs::canonicalize(launch.sandbox.home.join("dotfiles")).unwrap();
    let clone = launch.sandbox.home.join("IdeaProjects/.parallel/dotfiles/p1");
    fs::create_dir_all(&clone).unwrap();
    assert!(Command::new("git").args(["init", "-q"]).arg(&clone).status().unwrap().success());
    let read_live = format!("--allow-read-path {} ", live.display());
    let output = launch.sandbox.command(bin().join("greywall-agents/claude"), &[]).current_dir(&clone).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let started = launch.sandbox.calls("greywall").pop().unwrap();
    assert!(started.contains(&read_live), "{started}");
    assert!(!launch.run(&[]).contains(&read_live), "proj-a reads the live checkout");
}

#[test]
fn sandboxes_mount_the_report_sockets_folder_never_the_socket_alone() {
    // A socket file mounted alone stays the old one in every running sandbox once the socket is made
    // again: on 2026-10-19 a socket restart cut every session off the daemon.
    let profile: serde_json::Value = serde_json::from_str(&fs::read_to_string(repo().join("config/greywall/.config/greywall/greywall.json")).unwrap()).unwrap();
    let writable: Vec<&str> = profile["filesystem"]["allowWrite"].as_array().unwrap().iter().filter_map(|path| path.as_str()).collect();
    assert!(writable.contains(&"/run/user/1000/claude-report"), "{writable:?}");
    assert!(writable.iter().all(|path| !path.ends_with(".sock")), "{writable:?}");
    let unit = fs::read_to_string(repo().join("config/systemd/.config/systemd/user/osd-report.socket")).unwrap();
    assert!(unit.contains("ListenStream=%t/claude-report/osd-report.sock"), "{unit}");
}

#[test]
fn the_launcher_makes_the_report_sockets_folder_before_the_sandbox() {
    // Greywall mounts only what exists when it starts.
    let launch = Launch::new();
    launch.run(&[]);
    assert!(launch.sandbox.runtime.join("claude-report").is_dir());
}
