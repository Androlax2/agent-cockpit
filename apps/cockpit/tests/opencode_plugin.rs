//! opencode's cockpit plugin (config/opencode/.config/opencode/plugins/cockpit.ts, opencode 2's
//! plugin API): opencode's events become the Claude hook payloads session-state.sh takes. Run under
//! node, which strips the types itself and has the spawn the plugin uses; the hook is a fake that
//! records each payload.

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

/// Loads the plugin as opencode 2 does (its default export's setup, with a context) and plays the
/// steps in order: {"event": …} on ctx.event.subscribe's stream, {"tool": …} through the
/// execute.after hook it registered. Logs go to stderr.
const HARNESS: &str = r#"
import { appendFileSync } from 'node:fs'
const [plugin, directory, steps] = process.argv.slice(2)
const { default: definition } = await import(plugin)
if (typeof definition?.id !== "string" || typeof definition?.setup !== "function") throw new Error("no {id, setup} default export")
const pending = []
let wake = () => {}
let ended = false
async function* stream() {
  while (true) {
    while (pending.length) yield pending.shift()
    if (ended) return
    await new Promise((resolve) => (wake = resolve))
  }
}
const toolHooks = []
const ctx = {
  location: { directory },
  event: { subscribe: () => stream() },
  tool: { hook: async (name, callback) => { if (name === "execute.after") toolHooks.push(callback); return { dispose: async () => {} } } },
  session: {
    get: async ({sessionID}) => {
      if (process.env.SHIM_SESSION_ERROR) throw new Error('database unreadable')
      if (process.env.SHIM_EXISTING_MODEL) return {id: sessionID, model: {providerID: 'anthropic', id: 'existing'}}
      throw {_tag: process.env.SHIM_NOT_FOUND_TAG || 'SessionNotFoundError', sessionID}
    },
    create: async (input) => { appendFileSync(`${process.env.HOME}/created`, JSON.stringify(input) + '\n'); return input },
  },
}
const cleanup = await definition.setup(ctx)
const settle = () => new Promise((resolve) => setTimeout(resolve, 10))
for (const step of JSON.parse(steps)) {
  if (step.event) { pending.push(step.event); wake() }
  else for (const hook of toolHooks) await hook(step.tool)
  await settle()
}
ended = true
wake()
await settle()
if (cleanup) await cleanup()
"#;

struct Run {
    payloads: Vec<Value>,
    agents: Vec<String>,
    stderr: String,
    created: Vec<Value>,
}

fn run(steps: Value, hook_body: &str) -> Run {
    run_with(steps, hook_body, &[])
}

fn run_with(steps: Value, hook_body: &str, extra: &[(&str, &str)]) -> Run {
    let sandbox = Sandbox::new();
    write_executable(&sandbox.home.join("dotfiles/config/claude/.claude/hooks/session-state.sh"),
                     &format!("#!/usr/bin/env bash\n{{ cat; echo; }} >> \"$HOME/payloads\"\necho \"$COCKPIT_AGENT\" >> \"$HOME/agents\"\n{hook_body}\n"));
    let harness = sandbox.root.join("harness.mjs");
    fs::write(&harness, HARNESS).unwrap();
    let output = sandbox.command("node", extra).env_remove("CLAUDE_CODE_SESSION_ID").env_remove("COCKPIT_OPENCODE_MODEL").envs(extra.iter().copied())
        .arg(&harness).arg(plugin()).arg("/x/proj").arg(steps.to_string()).output()
        .expect("node runs the plugin's tests (pacman -S nodejs)");
    assert!(output.status.success(), "{}", stderr(&output));
    let read = |name: &str| fs::read_to_string(sandbox.home.join(name)).unwrap_or_default();
    Run {
        payloads: read("payloads").lines().map(|line| serde_json::from_str(line).unwrap()).collect(),
        agents: read("agents").lines().map(str::to_string).collect(),
        stderr: stderr(&output),
        created: read("created").lines().map(|line| serde_json::from_str(line).unwrap()).collect(),
    }
}

/// An opencode 2 bus event, as ctx.event.subscribe yields it.
fn event(kind: &str, data: Value) -> Value {
    json!({"event": {"id": "evt_1", "created": 0, "type": kind, "data": data}})
}

fn created(id: &str, parent: Option<&str>) -> Value {
    let mut data = json!({"sessionID": id, "location": {"directory": "/x/proj"}});
    if let Some(parent) = parent {
        data["parentID"] = json!(parent);
    }
    event("session.created", data)
}

fn prompt(session: &str, text: &str) -> Value {
    event("session.inbox.enqueued", json!({"sessionID": session, "inboxID": "inb_1", "item": {"type": "user", "payload": {"text": text}, "delivery": "queue"}}))
}

fn tool(session: &str) -> Value {
    json!({"tool": {"tool": "shell", "sessionID": session, "agent": "build", "messageID": "msg_1", "id": "call_1", "input": {}, "status": "completed", "result": {}}})
}

fn finished(session: &str) -> Value {
    event("session.execution.succeeded", json!({"sessionID": session}))
}

fn names(run: &Run) -> Vec<&str> {
    run.payloads.iter().map(|payload| payload["hook_event_name"].as_str().unwrap()).collect()
}

fn events_of(run: &Run) -> Vec<(String, String)> {
    run.payloads.iter().map(|payload| (payload["hook_event_name"].as_str().unwrap().to_string(), payload["session_id"].as_str().unwrap().to_string())).collect()
}

#[test]
fn a_turn_reports_like_claudes_hooks_do() {
    let run = run(json!([
        created(ROOT, None),
        prompt(ROOT, "add the chip"),
        tool(ROOT),
        event("permission.asked", json!({"id": "per_1", "sessionID": ROOT, "action": "shell", "resources": ["rm *"], "message": "run rm *"})),
        finished(ROOT),
    ]), "");
    assert_eq!(names(&run), ["SessionStart", "UserPromptSubmit", "PostToolUse", "Notification", "Stop"]);
    assert_eq!(run.payloads[1]["prompt"], "add the chip");
    assert_eq!((&run.payloads[3]["notification_type"], &run.payloads[3]["message"]), (&json!("permission_prompt"), &json!("opencode needs your permission: run rm *")));
    assert!(run.payloads.iter().all(|payload| payload["session_id"] == ROOT && payload["cwd"] == "/x/proj"), "{:?}", run.payloads);
    assert!(run.agents.iter().all(|agent| agent == "opencode"), "{:?}", run.agents);
}

#[test]
fn a_failed_or_interrupted_turn_ends_it_too() {
    for kind in ["session.execution.failed", "session.execution.interrupted"] {
        let run = run(json!([created(ROOT, None), event(kind, json!({"sessionID": ROOT, "reason": "user", "error": {"type": "x", "message": "y"}}))]), "");
        assert_eq!(names(&run), ["SessionStart", "Stop"], "{kind}");
    }
}

#[test]
fn a_question_needs_you() {
    let run = run(json!([created(ROOT, None), event("form.created", json!({"form": {"id": "frm_1", "sessionID": ROOT, "title": "Which one?", "fields": []}}))]), "");
    assert_eq!(run.payloads[1]["notification_type"], "elicitation_dialog");
}

#[test]
fn a_subagents_session_shows_on_its_roots_card_and_never_ends_the_turn() {
    let run = run(json!([
        created(ROOT, None),
        created(CHILD, Some(ROOT)),
        tool(CHILD),
        event("permission.asked", json!({"id": "per_2", "sessionID": CHILD, "action": "edit", "resources": []})),
        finished(CHILD),
        prompt(CHILD, "subtask prompt"),
    ]), "");
    let expected: Vec<(String, String)> = [("SessionStart", ROOT), ("PostToolUse", ROOT), ("Notification", ROOT)]
        .iter().map(|(name, session)| (name.to_string(), session.to_string())).collect();
    assert_eq!(events_of(&run), expected);
}

#[test]
fn a_resumed_session_starts_at_its_first_event() {
    let run = run(json!([prompt(ROOT, "go on")]), "");
    assert_eq!(names(&run), ["SessionStart", "UserPromptSubmit"]);
}

#[test]
fn the_cockpits_session_shows_from_the_start() {
    // The cockpit picks the id (--session) and exports it: opencode makes the session only at the
    // first prompt, the card is there before.
    let run = run_with(json!([prompt(ROOT, "hello")]), "", &[("CLAUDE_CODE_SESSION_ID", ROOT)]);
    assert_eq!(events_of(&run)[0], ("SessionStart".to_string(), ROOT.to_string()));
    assert_eq!(names(&run), ["SessionStart", "UserPromptSubmit"]);
}

#[test]
fn session_creation_is_left_to_the_launcher_before_the_interface_starts() {
    for reference in ["", "openai/gpt-6.1-sol#high"] {
        let run = run_with(json!([]), "", &[("CLAUDE_CODE_SESSION_ID", ROOT), ("COCKPIT_OPENCODE_MODEL", reference)]);
        assert!(run.created.is_empty());
        assert_eq!(events_of(&run), [("SessionStart".into(), ROOT.into())]);
    }
}

#[test]
fn reloading_the_plugin_preserves_an_existing_conversations_model() {
    let run = run_with(json!([]), "", &[
        ("CLAUDE_CODE_SESSION_ID", ROOT), ("COCKPIT_OPENCODE_MODEL", "openai/gpt-6.1-sol#high"), ("SHIM_EXISTING_MODEL", "1"),
    ]);
    assert!(run.created.is_empty());
}

#[test]
fn a_deleted_session_is_gone() {
    let run = run(json!([created(ROOT, None), event("session.deleted", json!({"sessionID": ROOT}))]), "");
    assert_eq!(names(&run), ["SessionStart", "SessionEnd"]);
}

#[test]
fn a_failing_hook_is_logged_not_swallowed() {
    let run = run(json!([created(ROOT, None)]), "echo 'jq: not found' >&2; exit 1");
    assert!(run.stderr.contains("session-state.sh SessionStart exited 1: jq: not found"), "{}", run.stderr);
}

/// opencode.jsonc as JSON: its whole-line // comments left out.
fn opencode_config() -> Value {
    let text = fs::read_to_string(repo().join("config/opencode/.config/opencode/opencode.jsonc")).unwrap();
    let json: String = text.lines().filter(|line| !line.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    serde_json::from_str(&json).expect("opencode.jsonc is JSON once its comment lines are left out")
}

#[test]
fn opencode_registers_figcraft_app_on_the_port_the_launcher_forwards() {
    let server = &opencode_config()["mcp"]["figcraft-app"];
    let sandbox = Sandbox::new();
    sandbox.shim("figcraft-cli", "exit 0");
    for port in ["17979", "17980"] {
        let arguments = server["command"].as_array().unwrap();
        let mut command = sandbox.command(arguments[0].as_str().unwrap(), &[("FIGCRAFT_CONTROL_PORT", port)]);
        for argument in &arguments[1..] { command.arg(argument.as_str().unwrap()); }
        assert!(command.output().unwrap().status.success());
        assert_eq!(sandbox.calls("figcraft-cli").last().unwrap(), &format!("figcraft-cli mcp --attach {port}"));
    }
    let arguments = server["command"].as_array().unwrap();
    let mut command = sandbox.command(arguments[0].as_str().unwrap(), &[("FIGCRAFT_CONTROL_PORT", "")]);
    for argument in &arguments[1..] { command.arg(argument.as_str().unwrap()); }
    assert!(!command.output().unwrap().status.success(), "connected to an implicit default port");
}

#[test]
fn opencode_leaves_updates_to_opencode_update() {
    assert_eq!(opencode_config()["autoupdate"], json!(false));
}
