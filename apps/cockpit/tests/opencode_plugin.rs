//! opencode's cockpit plugin (config/opencode/.config/opencode/plugins/cockpit.ts): opencode's
//! events become the Claude hook payloads session-state.sh takes. Run under node, which strips the
//! types itself and has the spawn the plugin uses; the hook is a fake that records each payload.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

const ROOT: &str = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L";
const CHILD: &str = "ses_3f2a9c0b2000AbCdEfGhIjKlMn";

fn plugin() -> PathBuf {
    repo().join("config/opencode/.config/opencode/plugins/cockpit.ts")
}

/// Loads the plugin as opencode does and calls its hooks in order: {"hook": "event", "event": …}
/// or {"hook": name, "input": …, "output": …}. Prints what shell.env set, and logs to stderr.
const HARNESS: &str = r#"
const [plugin, directory, calls] = process.argv.slice(2)
const { CockpitPlugin } = await import(plugin)
const client = { app: { log: async (entry) => console.error(JSON.stringify(entry.body)) } }
const hooks = await CockpitPlugin({ client, directory })
for (const call of JSON.parse(calls)) {
  if (call.hook === "event") {
    await hooks.event({ event: call.event })
  } else {
    const output = call.output ?? { env: {} }
    await hooks[call.hook](call.input, output)
    if (call.hook === "shell.env") console.log(JSON.stringify(output.env))
  }
}
"#;

struct Run {
    payloads: Vec<Value>,
    agents: Vec<String>,
    stdout: String,
    stderr: String,
}

fn run(calls: Value, hook_body: &str) -> Run {
    let sandbox = Sandbox::new();
    write_executable(&sandbox.home.join(".claude/hooks/session-state.sh"),
                     &format!("#!/usr/bin/env bash\n{{ cat; echo; }} >> \"$HOME/payloads\"\necho \"$CLAUDE_SESSIONS_AGENT\" >> \"$HOME/agents\"\n{hook_body}\n"));
    let harness = sandbox.root.join("harness.mjs");
    fs::write(&harness, HARNESS).unwrap();
    let output = sandbox.command("node", &[]).arg(&harness).arg(plugin()).arg("/x/proj").arg(calls.to_string()).output()
        .expect("node runs the plugin's tests (pacman -S nodejs)");
    assert!(output.status.success(), "{}", stderr(&output));
    let read = |name: &str| fs::read_to_string(sandbox.home.join(name)).unwrap_or_default();
    Run {
        payloads: read("payloads").lines().map(|line| serde_json::from_str(line).unwrap()).collect(),
        agents: read("agents").lines().map(str::to_string).collect(),
        stdout: stdout(&output),
        stderr: stderr(&output),
    }
}

fn event(kind: &str, properties: Value) -> Value {
    json!({"hook": "event", "event": {"type": kind, "properties": properties}})
}

fn created(id: &str, parent: Option<&str>) -> Value {
    let mut info = json!({"id": id, "directory": "/x/proj"});
    if let Some(parent) = parent {
        info["parentID"] = json!(parent);
    }
    event("session.created", json!({"info": info}))
}

fn prompt(session: &str, texts: &[(&str, bool)]) -> Value {
    let parts: Vec<Value> = texts.iter().map(|(text, synthetic)| json!({"type": "text", "text": text, "synthetic": synthetic})).collect();
    json!({"hook": "chat.message", "input": {"sessionID": session}, "output": {"message": {}, "parts": parts}})
}

fn tool(session: &str) -> Value {
    json!({"hook": "tool.execute.after", "input": {"tool": "bash", "sessionID": session, "callID": "c1"}, "output": {}})
}

fn events_of(run: &Run) -> Vec<(String, String)> {
    run.payloads.iter().map(|payload| (payload["hook_event_name"].as_str().unwrap().to_string(), payload["session_id"].as_str().unwrap().to_string())).collect()
}

#[test]
fn a_turn_reports_like_claudes_hooks_do() {
    let run = run(json!([
        created(ROOT, None),
        prompt(ROOT, &[("add the chip", false), ("<file contents>", true)]),
        tool(ROOT),
        event("permission.asked", json!({"id": "per_1", "sessionID": ROOT, "permission": "bash", "patterns": ["rm *"]})),
        event("session.idle", json!({"sessionID": ROOT})),
    ]), "");
    let names: Vec<&str> = run.payloads.iter().map(|payload| payload["hook_event_name"].as_str().unwrap()).collect();
    assert_eq!(names, ["SessionStart", "UserPromptSubmit", "PostToolUse", "Notification", "Stop"]);
    assert_eq!(run.payloads[1]["prompt"], "add the chip", "only what was typed names the task");
    assert_eq!((&run.payloads[3]["notification_type"], &run.payloads[3]["message"]), (&json!("permission_prompt"), &json!("opencode needs your permission: bash")));
    assert!(run.payloads.iter().all(|payload| payload["session_id"] == ROOT && payload["cwd"] == "/x/proj"), "{:?}", run.payloads);
    assert!(run.agents.iter().all(|agent| agent == "opencode"), "{:?}", run.agents);
}

#[test]
fn a_question_needs_you() {
    let run = run(json!([created(ROOT, None), event("question.asked", json!({"id": "que_1", "sessionID": ROOT}))]), "");
    assert_eq!(run.payloads[1]["notification_type"], "elicitation_dialog");
}

#[test]
fn a_subagents_session_shows_on_its_roots_card_and_never_ends_the_turn() {
    let run = run(json!([
        created(ROOT, None),
        created(CHILD, Some(ROOT)),
        tool(CHILD),
        event("permission.asked", json!({"id": "per_2", "sessionID": CHILD, "permission": "edit"})),
        event("session.idle", json!({"sessionID": CHILD})),
        prompt(CHILD, &[("subtask prompt", false)]),
    ]), "");
    let expected: Vec<(String, String)> = [("SessionStart", ROOT), ("PostToolUse", ROOT), ("Notification", ROOT)]
        .iter().map(|(name, session)| (name.to_string(), session.to_string())).collect();
    assert_eq!(events_of(&run), expected);
}

#[test]
fn a_resumed_session_starts_at_its_first_event() {
    // opencode --session <id> creates no session: no session.created comes.
    let run = run(json!([prompt(ROOT, &[("go on", false)])]), "");
    let names: Vec<&str> = run.payloads.iter().map(|payload| payload["hook_event_name"].as_str().unwrap()).collect();
    assert_eq!(names, ["SessionStart", "UserPromptSubmit"]);
}

#[test]
fn a_deleted_session_is_gone() {
    let run = run(json!([created(ROOT, None), event("session.deleted", json!({"info": {"id": ROOT}}))]), "");
    let names: Vec<&str> = run.payloads.iter().map(|payload| payload["hook_event_name"].as_str().unwrap()).collect();
    assert_eq!(names, ["SessionStart", "SessionEnd"]);
}

#[test]
fn the_agents_commands_carry_their_sessions_id() {
    // ask-to-run, ask-to-guide and claude-todo name the session from CLAUDE_CODE_SESSION_ID.
    let run = run(json!([created(ROOT, None), created(CHILD, Some(ROOT)),
                         {"hook": "shell.env", "input": {"cwd": "/x/proj", "sessionID": CHILD}}]), "");
    let environment: Value = serde_json::from_str(run.stdout.trim()).unwrap();
    assert_eq!(environment["CLAUDE_CODE_SESSION_ID"], ROOT);
}

#[test]
fn a_failing_hook_is_logged_not_swallowed() {
    let run = run(json!([created(ROOT, None)]), "echo 'jq: not found' >&2; exit 1");
    assert!(run.stderr.contains("session-state.sh SessionStart exited 1: jq: not found"), "{}", run.stderr);
}
