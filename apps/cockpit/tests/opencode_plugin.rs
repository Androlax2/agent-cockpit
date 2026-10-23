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
const sessionHooks = new Map()
let messages = []
let connection
let lookupError = false
let switchedConnection
const ctx = {
  location: { directory },
  event: { subscribe: () => stream() },
  tool: { hook: async (name, callback) => { if (name === "execute.after") toolHooks.push(callback); return { dispose: async () => {} } } },
  session: {
    hook: async (name, callback) => { sessionHooks.set(name, callback); return { dispose: async () => sessionHooks.delete(name) } },
    context: async () => { if (lookupError) throw new Error('metadata unavailable'); if (switchedConnection) connection = switchedConnection; return messages },
    get: async ({sessionID}) => {
      if (process.env.SHIM_SESSION_ERROR) throw new Error('database unreadable')
      if (process.env.SHIM_EXISTING_MODEL) return {id: sessionID, model: {providerID: 'anthropic', id: 'existing'}}
      throw {_tag: process.env.SHIM_NOT_FOUND_TAG || 'SessionNotFoundError', sessionID}
    },
    create: async (input) => { appendFileSync(`${process.env.HOME}/created`, JSON.stringify(input) + '\n'); return input },
  },
  integration: { connection: { active: async () => { if (lookupError) throw new Error('metadata unavailable'); return connection } } },
}
const cleanup = await definition.setup(ctx)
const settle = () => new Promise((resolve) => setTimeout(resolve, 10))
for (const step of JSON.parse(steps)) {
  if (step.request) {
    messages = step.messages ?? []
    connection = step.connection
    lookupError = step.lookupError ?? false
    switchedConnection = step.switchConnection
    const request = new Request(step.request.url ?? 'https://api.anthropic.com/', { headers: step.request.headers ?? {} })
    const before = JSON.stringify([...request.headers])
    const input = {sessionID: step.request.sessionID, model: step.request.model, kind: step.request.kind ?? 'primary', request}
    const callback = sessionHooks.get('http.request')
    if (callback) await callback(input)
    if (JSON.stringify([...request.headers]) !== before) throw new Error('billing capture changed request headers')
  }
  else if (step.event) { pending.push(step.event); wake() }
  else for (const hook of toolHooks) await hook(step.tool)
  await settle()
}
ended = true
wake()
await settle()
if (cleanup) await cleanup()
if (sessionHooks.size) throw new Error('session hook was not disposed')
"#;

struct Run {
    payloads: Vec<Value>,
    agents: Vec<String>,
    stderr: String,
    created: Vec<Value>,
    billing: Vec<Value>,
}

fn run(steps: Value, hook_body: &str) -> Run {
    run_with(steps, hook_body, &[])
}

fn run_with(steps: Value, hook_body: &str, extra: &[(&str, &str)]) -> Run {
    let sandbox = Sandbox::new();
    if extra.iter().any(|(key, value)| *key == "SHIM_BILLING_FAILURE" && *value == "1") {
        fs::create_dir_all(sandbox.home.join(".cache/claude")).unwrap();
        fs::write(sandbox.home.join(".cache/claude/billing"), "not a directory").unwrap();
    }
    write_executable(&sandbox.home.join("agent-cockpit/config/claude/.claude/hooks/session-state.sh"),
                      &format!("#!/usr/bin/env bash\n{{ cat; echo; }} >> \"$HOME/payloads\"\necho \"$COCKPIT_AGENT\" >> \"$HOME/agents\"\n{hook_body}\n"));
    fs::copy(hook().with_file_name("billing-capture.py"), sandbox.home.join("agent-cockpit/config/claude/.claude/hooks/billing-capture.py")).unwrap();
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
        billing: fs::read_dir(sandbox.home.join(".cache/claude/billing")).into_iter().flatten().map(|entry|
            serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap()).collect(),
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

fn shell_created(id: &str, session: &str) -> Value {
    event("shell.created", json!({"info": {"id": id, "status": "running", "command": "make test", "cwd": "/x/proj", "shell": "/bin/bash", "file": "/x/output", "metadata": {"sessionID": session}, "time": {"started": 1}}}))
}

#[test]
fn background_shells_are_reported_until_the_last_one_exits() {
    let run = run(json!([
        created(ROOT, None),
        shell_created("sh_1", ROOT),
        shell_created("sh_2", ROOT),
        finished(ROOT),
        event("shell.exited", json!({"id": "sh_1", "status": "exited", "exit": 0})),
        event("shell.exited", json!({"id": "sh_2", "status": "timeout"})),
    ]), "");
    let stop = run.payloads.iter().find(|payload| payload["hook_event_name"] == "Stop").unwrap();
    assert_eq!(stop["background_tasks"].as_array().map(Vec::len), Some(2));
    let updates: Vec<_> = run.payloads.iter().filter(|payload| payload["notification_type"] == "background_work").collect();
    assert_eq!(updates.iter().map(|payload| payload["background_tasks"].as_array().unwrap().len()).collect::<Vec<_>>(), [1, 2, 1, 0]);
}

#[test]
fn a_late_shell_exit_does_not_restart_a_deleted_session() {
    let run = run(json!([
        created(ROOT, None), created(CHILD, Some(ROOT)),
        shell_created("sh_1", ROOT), shell_created("sh_2", CHILD),
        event("session.deleted", json!({"sessionID": ROOT})),
        event("shell.exited", json!({"id": "sh_1", "status": "killed"})),
        event("shell.deleted", json!({"id": "sh_2"})),
    ]), "");
    assert_eq!(names(&run), ["SessionStart", "Notification", "Notification", "SessionEnd"]);
}

#[test]
fn a_childs_shell_is_reported_on_the_root_and_deletion_clears_it() {
    let run = run(json!([
        created(ROOT, None), created(CHILD, Some(ROOT)),
        shell_created("sh_1", CHILD), finished(ROOT),
        event("shell.deleted", json!({"id": "sh_1"})),
        event("shell.exited", json!({"id": "sh_unrelated", "status": "killed"})),
    ]), "");
    let updates: Vec<_> = run.payloads.iter().filter(|payload| payload["notification_type"] == "background_work").collect();
    assert_eq!(updates.len(), 2);
    assert!(updates.iter().all(|payload| payload["session_id"] == ROOT));
    assert_eq!(updates[1]["background_tasks"], json!([]));
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

fn billing_request(provider: &str, message: &str, method: &str) -> Value {
    let model = if provider == "openai" { "gpt-6.1-sol" } else { "claude-opus-5-5" };
    let hostname = if provider == "openai" && method == "oauth" { "chatgpt.com".into() } else { format!("api.{provider}.com") };
    json!({"request": {"sessionID":ROOT, "model":{"providerID":provider,"id":model},
        "url":format!("https://{hostname}/"), "headers": if provider == "anthropic" && method == "key" {json!({"x-api-key":"not-a-real-key"})} else {json!({"authorization":"Bearer test-only"})}},
        "connection":{"type":"credential","id":"credential-test","label":"test","method":method},
        "messages":[{"type":"user","id":format!("input-{message}"),"time":{"created":1},"text":"test"}]})
}

fn started_reply(provider: &str, id: &str, session: &str) -> Value {
    event("session.step.started", json!({"sessionID":session,"assistantMessageID":id,"agent":"build",
        "model":{"providerID":provider,"id":if provider == "openai" { "gpt-6.1-sol" } else { "claude-opus-5-5" }},"started":1}))
}

#[test]
fn funding_is_recorded_for_each_exact_reply_across_provider_and_billing_switches() {
    let run = run(json!([
        billing_request("anthropic", "msg_api", "key"),
        started_reply("anthropic", "msg_api", ROOT),
        billing_request("openai", "msg_plan", "oauth"),
        started_reply("openai", "msg_plan", ROOT),
        billing_request("anthropic", "msg_plan_anthropic", "oauth"),
        started_reply("anthropic", "msg_plan_anthropic", ROOT),
    ]), "");
    let modes: std::collections::HashMap<_, _> = run.billing.iter().map(|record|
        (record["key"].as_str().unwrap(), record["billing"].as_str().unwrap())).collect();
    assert_eq!(modes, std::collections::HashMap::from([
        ("opencode:msg_api", "api"), ("opencode:msg_plan", "subscription"), ("opencode:msg_plan_anthropic", "unknown")]));
    assert!(run.billing.iter().all(|record| record.as_object().unwrap().len() == 2));
    assert!(!serde_json::to_string(&run.billing).unwrap().contains("not-a-real-key"));
}

#[test]
fn an_openai_environment_api_key_is_not_mistaken_for_the_subscription_connection() {
    let mut request = billing_request("openai", "msg_env", "key");
    request["connection"] = json!({"type":"env","name":"OPENAI_API_KEY"});
    request["request"]["headers"] = json!({"authorization":"Bearer not-a-real-key"});
    let run = run(json!([request, started_reply("openai", "msg_env", ROOT)]), "");
    assert_eq!(run.billing, vec![json!({"key":"opencode:msg_env","billing":"api"})]);
}

#[test]
fn funding_is_not_guessed_when_message_or_authentication_evidence_is_ambiguous() {
    let base = billing_request("anthropic", "msg_ambiguous", "key");
    let mut external = base.clone(); external["connection"]["method"] = json!("external"); external["request"]["headers"] = json!({});
    let mut custom = base.clone(); custom["request"]["model"]["providerID"] = json!("custom");
    let mut gateway = base.clone(); gateway["request"]["url"] = json!("https://example.invalid/");
    let mut insecure = base.clone(); insecure["request"]["url"] = json!("http://api.anthropic.com/");
    let mut auxiliary = base.clone(); auxiliary["request"]["kind"] = json!("title");
    for request in [external, custom, gateway, insecure, auxiliary] {
        let run = run(json!([request, started_reply("anthropic", "msg_ambiguous", ROOT)]), "");
        assert!(run.billing.iter().all(|record| record["billing"] == "unknown"), "{:?}", run.billing);
    }
}

#[test]
fn a_subagents_reply_gets_its_own_funding_key() {
    let mut request = billing_request("anthropic", "msg_child", "key");
    request["request"]["sessionID"] = json!(CHILD);
    let run = run(json!([created(ROOT, None), created(CHILD, Some(ROOT)), request, started_reply("anthropic", "msg_child", CHILD)]), "");
    assert_eq!(run.billing, vec![json!({"key":"opencode:msg_child","billing":"api"})]);
}

#[test]
fn billing_capture_failure_is_logged_without_breaking_the_plugin() {
    let run = run_with(json!([billing_request("anthropic", "msg_failed", "key"), started_reply("anthropic", "msg_failed", ROOT)]), "", &[("SHIM_BILLING_FAILURE", "1")]);
    assert!(run.stderr.contains("billing capture failed"), "{}", run.stderr);
    assert!(run.billing.is_empty());
}

#[test]
fn changing_the_active_connection_cannot_relabel_an_assembled_bearer_request() {
    let mut request = billing_request("anthropic", "msg_race", "oauth");
    request["switchConnection"] = json!({"type":"credential","id":"new-key","label":"changed while context was read","method":"key"});
    let run = run(json!([request, started_reply("anthropic", "msg_race", ROOT)]), "");
    assert!(run.billing.iter().all(|record| record["billing"] == "unknown"), "ambiguous Bearer request was relabeled: {:?}", run.billing);
}

#[test]
fn dispatch_waits_for_the_native_reply_id_rather_than_guessing_one_from_context() {
    let request = billing_request("anthropic", "decoy", "key");
    let no_reply = run(json!([request.clone()]), "");
    assert!(no_reply.billing.is_empty());
    let with_reply = run(json!([request, started_reply("anthropic", "msg_native_id", ROOT)]), "");
    assert_eq!(with_reply.billing, vec![json!({"key":"opencode:msg_native_id","billing":"api"})]);
}

#[test]
fn conflicting_retries_are_not_confidently_attributed_to_either_billing_mode() {
    let run = run(json!([billing_request("anthropic", "retry", "key"), billing_request("anthropic", "retry", "oauth"),
        started_reply("anthropic", "msg_retried", ROOT)]), "");
    assert_eq!(run.billing, vec![json!({"key":"opencode:msg_retried","billing":"unknown"})]);
}
