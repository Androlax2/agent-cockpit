//! The agents' interface: one row per agent in cockpit::agents, one file per agent for
//! claude-sessions, every verb defined, and an agent added as a file is run like the others.

mod common;
use common::sandbox::*;

use cockpit::agents::{self, Feature};
use cockpit::sessions::START_FILES;
use std::fs;
use std::path::PathBuf;

fn agents_dir() -> PathBuf {
    bin().join("claude-sessions-agents")
}

/// The names of the agent files, sorted.
fn agent_files() -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(agents_dir()).unwrap().flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().strip_suffix(".sh").map(str::to_string)).collect();
    names.sort();
    names
}

#[test]
fn every_agent_has_a_row_and_a_file_for_claude_sessions() {
    let mut rows = agents::names();
    rows.sort();
    assert_eq!(rows, agent_files());
}

#[test]
fn every_agent_runs_in_its_own_greywall_launcher_and_profile() {
    for name in agents::names() {
        assert!(bin().join("greywall-agents").join(name).is_file(), "no launcher greywall-agents/{name}");
        assert!(repo().join(format!("config/greywall/.config/greywall/learned/{name}.json")).is_file(), "no greywall profile for {name}");
    }
}

#[test]
fn every_start_file_is_read_by_an_agent_and_every_agent_reads_known_ones() {
    let labels: Vec<&str> = START_FILES.iter().map(|(label, _)| *label).collect();
    for label in &labels {
        assert!(agents::AGENTS.iter().any(|agent| agent.start_files.contains(label)), "no agent reads {label}");
    }
    for agent in &agents::AGENTS {
        for label in agent.start_files {
            assert!(labels.contains(label), "{} reads {label}, which is not in START_FILES", agent.name);
        }
    }
}

#[test]
fn the_default_agent_has_a_row_and_no_badge() {
    let default = agents::find(agents::DEFAULT).expect("the default agent's row");
    assert!(default.badge.is_none());
    assert!(agents::AGENTS.iter().filter(|agent| agent.name != agents::DEFAULT).all(|agent| agent.badge.is_some()));
}

#[test]
fn a_session_id_belongs_to_its_agent_and_a_tmux_name_to_the_longest_prefix() {
    assert!(agents::is_session_id(SESSION_A));
    assert!(agents::is_session_id(OPENCODE_SESSION));
    assert!(!agents::is_session_id("claude-retry-opencode-1"));
    assert!(!agents::is_session_id("../etc"));
    assert_eq!(agents::of_tmux_session("claude-retry-opencode-17").map(|agent| agent.name), Some("opencode"));
    assert_eq!(agents::of_tmux_session("claude-retry-bg-17").map(|agent| agent.name), Some("claude"));
    assert!(agents::of_tmux_session("other").is_none());
}

#[test]
fn claude_code_has_every_feature_and_opencode_none() {
    let features = [Feature::Effort, Feature::Billing, Feature::Compact, Feature::RemoteControl, Feature::Dialogs, Feature::Interrupt, Feature::ScreenStates];
    assert!(features.iter().all(|feature| agents::get("claude").can(*feature)));
    assert!(features.iter().all(|feature| !agents::get("opencode").can(*feature)));
}

/// A copy of the agent files in the sandbox, plus `extra` as <name>.sh.
fn agents_with(sandbox: &Sandbox, name: &str, extra: &str) -> String {
    let folder = sandbox.root.join("agents");
    fs::create_dir_all(&folder).unwrap();
    for file in agent_files() {
        fs::copy(agents_dir().join(format!("{file}.sh")), folder.join(format!("{file}.sh"))).unwrap();
    }
    fs::write(folder.join(format!("{name}.sh")), extra).unwrap();
    folder.display().to_string()
}

/// opencode's file as another agent's: every name in it its own (agent files share one namespace).
fn opencode_as(name: &str) -> String {
    fs::read_to_string(agents_dir().join("opencode.sh")).unwrap()
        .replace("opencode", name).replace("OPENCODE", &name.to_uppercase())
}

#[test]
fn every_name_an_agent_file_defines_carries_its_agents_name() {
    let definition = regex::Regex::new(r"(?m)^([A-Za-z_][A-Za-z0-9_]*)(?:\(\)|=)").unwrap();
    for name in agent_files() {
        let text = fs::read_to_string(agents_dir().join(format!("{name}.sh"))).unwrap();
        for found in definition.captures_iter(&text) {
            let defined = &found[1];
            assert!(defined.to_lowercase().contains(&name), "{name}.sh defines {defined}, which does not carry its name");
        }
    }
}

#[test]
fn an_agent_file_missing_a_verb_stops_claude_sessions_and_names_it() {
    let sandbox = Sandbox::new();
    let folder = agents_with(&sandbox, "codex", &opencode_as("codex").replace("agent_codex_say()", "unrelated_say()"));
    let output = sandbox.run(bin().join("claude-sessions"), &["--agent", "codex", "background", "/tmp", "hi"], &[("CLAUDE_SESSIONS_AGENTS_DIR", &folder)]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("the agent codex") && stderr(&output).contains("does not define: say"), "{}", stderr(&output));
    assert!(sandbox.calls("tmux").is_empty());
}

#[test]
fn an_agent_added_as_a_file_starts_with_agent_and_its_sessions_go_to_it() {
    let sandbox = Sandbox::new();
    sandbox.shim("tmux", "");
    let folder = agents_with(&sandbox, "codex", &opencode_as("codex"));
    let project = sandbox.root.join("project");
    fs::create_dir_all(&project).unwrap();
    let output = sandbox.run(bin().join("claude-sessions"), &["--agent", "codex", "background", &project.display().to_string(), "fix the chip"],
                             &[("CLAUDE_SESSIONS_AGENTS_DIR", &folder)]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let start = sandbox.calls("tmux").into_iter().find(|call| call.contains("new-session")).expect("no start");
    assert!(start.contains("-s claude-retry-codex-") && start.ends_with("--prompt fix the chip"), "{start}");
}

#[test]
fn an_unknown_agent_is_refused_with_the_known_ones_named() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(bin().join("claude-sessions"), &["--agent", "codex", "new", "/tmp"], &[]);
    assert_eq!(output.status.code(), Some(2));
    assert!(stderr(&output).contains("--agent takes one of: claude opencode"), "{}", stderr(&output));
}

#[test]
fn left_knows_the_prompt_of_an_agent_added_as_a_file() {
    let sandbox = Sandbox::new();
    let marker = "agent_codex_prompt_start() {\n    printf '%s\\n' '^ > $'\n}\n";
    let codex = opencode_as("codex");
    let start = codex.find("agent_codex_prompt_start() {").unwrap();
    let end = start + codex[start..].find("\n}\n").unwrap() + 3;
    let folder = agents_with(&sandbox, "codex", &format!("{}{marker}{}", &codex[..start], &codex[end..]));
    sandbox.shim("tmux", r#"case $1 in
  display-message) echo "3 7" ;;
  capture-pane) printf '%s\n' ' > ' ;;
esac"#);
    let output = sandbox.run(bin().join("claude-sessions-left"), &["%11"], &[("CLAUDE_SESSIONS_AGENTS_DIR", &folder)]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(sandbox.calls("tmux").contains(&"tmux select-pane -L -t %11".to_string()), "{:?}", sandbox.calls("tmux"));
}
