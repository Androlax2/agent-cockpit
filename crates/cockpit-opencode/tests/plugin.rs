//! The plugin OpenCode loads, run by Node.js (OpenCode's Bun runs the same module): it
//! forwards the events that prove activity to the bridge, in order, one at a time, and stays
//! silent outside its launch's server. A fake bridge records what it receives. Node.js must
//! be on `PATH`; these tests fail without it rather than pass unrun.

mod common;

use std::process::Command;

use cockpit_opencode::PLUGIN_SOURCE;

const PORT: &str = "41731";

/// Runs the plugin in Node.js with `environment`, as the server on `server_port`, feeds it
/// `events` and returns what the fake bridge received: one `<cockpit> <event type> <session>`
/// line per run, after waiting for `expected` lines.
fn run_plugin(
    events: &[String],
    server_port: &str,
    environment: &[(&str, &str)],
    expected: usize,
) -> (Vec<String>, String) {
    let dir = tempfile::tempdir().unwrap();
    let plugin = dir.path().join("cockpit-opencode-plugin.mjs");
    std::fs::write(&plugin, PLUGIN_SOURCE).unwrap();
    let log = dir.path().join("bridge.log");
    // The busy status takes longest, so a bridge run out of order would show it last.
    common::script(
        dir.path(),
        "cockpit-opencode-bridge",
        "payload=$(cat)\n\
         case \"$payload\" in *'\"busy\"'*) sleep 0.3 ;; esac\n\
         type=$(printf '%s' \"$payload\" | sed 's/.*\"type\":\"\\([a-z.]*\\)\",\"properties\".*/\\1/')\n\
         session=$(printf '%s' \"$payload\" | sed 's/^[^}]*\"sessionID\":\"\\([a-zA-Z0-9_]*\\)\".*/\\1/')\n\
         printf '%s %s %s\\n' \"$1\" \"$type\" \"$session\" >> \"$BRIDGE_LOG\"",
    );
    let events_file = dir.path().join("events.json");
    std::fs::write(&events_file, format!("[{}]", events.join(","))).unwrap();
    let driver = dir.path().join("driver.mjs");
    std::fs::write(&driver, DRIVER).unwrap();
    let output = Command::new("node")
        .arg(&driver)
        .args([&plugin, &events_file, &log])
        .arg(server_port)
        .arg(expected.to_string())
        .env_remove("COCKPIT_OPENCODE_BRIDGE")
        .env_remove("COCKPIT_OPENCODE_COCKPIT")
        .env_remove("COCKPIT_OPENCODE_PORT")
        .env("BRIDGE_LOG", &log)
        .envs(environment.iter().map(|(name, value)| {
            let value = value.replace("{dir}", &dir.path().display().to_string());
            (*name, value)
        }))
        .output()
        .unwrap_or_else(|error| panic!("the plugin tests need Node.js on PATH: {error}"));
    assert!(output.status.success(), "{output:?}");
    let received = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    (received, String::from_utf8(output.stdout).unwrap())
}

/// What the bridge received from a launched plugin: `<cockpit> <event type>` per run.
fn run_launched(events: &[String], expected: usize) -> Vec<String> {
    run_plugin(events, PORT, &launched(), expected)
        .0
        .iter()
        .map(|line| line.rsplit_once(' ').unwrap().0.to_owned())
        .collect()
}

/// What the bridge received from a launched plugin: `<event type> <session>` per run.
fn run_plugin_recording_sessions(events: &[String], expected: usize) -> Vec<String> {
    run_plugin(events, PORT, &launched(), expected)
        .0
        .iter()
        .map(|line| line.split_once(' ').unwrap().1.to_owned())
        .collect()
}

/// Imports the plugin, starts it as the server on its port, feeds it each event, then waits
/// for the expected number of bridge runs and a little more for any extra one. Prints
/// `hooks: <names>`.
const DRIVER: &str = r#"
import { readFileSync, existsSync } from "node:fs"
const [plugin, events, log, port, expected] = process.argv.slice(2)
const { CockpitBridge } = await import(plugin)
const hooks = await CockpitBridge({ serverUrl: new URL(`http://127.0.0.1:${port}`) })
console.log(`hooks: ${Object.keys(hooks).join(",")}`)
for (const event of JSON.parse(readFileSync(events, "utf8"))) await hooks.event?.({ event })
const lines = () => (existsSync(log) ? readFileSync(log, "utf8").trim().split("\n").filter(Boolean).length : 0)
const until = Date.now() + 10000
while (lines() < Number(expected) && Date.now() < until) await new Promise((r) => setTimeout(r, 20))
await new Promise((r) => setTimeout(r, 400))
"#;

fn launched() -> Vec<(&'static str, &'static str)> {
    vec![
        ("COCKPIT_OPENCODE_BRIDGE", "{dir}/cockpit-opencode-bridge"),
        ("COCKPIT_OPENCODE_COCKPIT", "/opt/cockpit/bin/cockpit"),
        ("COCKPIT_OPENCODE_PORT", PORT),
    ]
}

fn fixtures(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| common::fixture(name)).collect()
}

#[test]
fn a_turn_is_forwarded_in_order_without_the_events_that_prove_nothing() {
    let events = fixtures(&[
        "session-created",
        "session-status-busy",
        "tool-running",
        "tool-completed",
        "message-assistant-tool-calls",
        "session-status-retry",
        "message-assistant-stop",
        "session-status-idle",
        "session-idle",
    ]);

    let received = run_launched(&events, 5);

    assert_eq!(
        received,
        [
            "/opt/cockpit/bin/cockpit session.created",
            "/opt/cockpit/bin/cockpit session.status",
            "/opt/cockpit/bin/cockpit message.part.updated",
            "/opt/cockpit/bin/cockpit message.updated",
            "/opt/cockpit/bin/cockpit session.status",
        ]
    );
}

#[test]
fn an_ended_message_is_forwarded_once() {
    let events = fixtures(&["message-assistant-stop", "message-assistant-stop"]);

    let received = run_launched(&events, 1);

    assert_eq!(received, ["/opt/cockpit/bin/cockpit message.updated"]);
}

#[test]
fn dialogs_and_failed_turns_are_forwarded() {
    let events = fixtures(&[
        "permission-asked",
        "question-asked",
        "tool-error",
        "message-assistant-aborted",
    ]);

    let received = run_launched(&events, 4);

    assert_eq!(received.len(), 4, "{received:?}");
}

#[test]
fn a_subagents_session_is_never_forwarded() {
    let child_busy = common::fixture("session-status-busy").replace(
        "ses_1c4f2e8a3ffeJ7qkR2mWbXzD0v",
        "ses_1c4f2d11affeQ0aZ9yTnLcVb3u",
    );
    let events = vec![common::fixture("session-created-subagent"), child_busy];

    let received = run_launched(&events, 0);

    assert_eq!(received, Vec::<String>::new());
}

/// A subagent's permission prompt waits in the same terminal, so it reaches the owner under
/// the root session.
#[test]
fn a_subagents_dialog_is_forwarded_under_its_root_session() {
    let child = "ses_1c4f2d11affeQ0aZ9yTnLcVb3u";
    let child_asks =
        common::fixture("permission-asked").replace("ses_1c4f2e8a3ffeJ7qkR2mWbXzD0v", child);
    let events = vec![common::fixture("session-created-subagent"), child_asks];

    let received = run_plugin_recording_sessions(&events, 1);

    assert_eq!(
        received,
        ["permission.asked ses_1c4f2e8a3ffeJ7qkR2mWbXzD0v"]
    );
}

/// An `opencode` a tool starts inherits the launch's environment, but runs another server.
#[test]
fn in_another_server_it_subscribes_to_nothing() {
    let events = fixtures(&["session-status-busy"]);

    let (received, stdout) = run_plugin(&events, "41732", &launched(), 0);

    assert_eq!(received, Vec::<String>::new());
    assert_eq!(stdout.trim(), "hooks:");
}

#[test]
fn without_its_launch_variables_it_subscribes_to_nothing() {
    let events = fixtures(&["session-status-busy"]);

    let (received, stdout) = run_plugin(&events, PORT, &[], 0);

    assert_eq!(received, Vec::<String>::new());
    assert_eq!(stdout.trim(), "hooks:");
}

#[test]
fn a_bridge_that_cannot_run_never_stops_the_plugin() {
    let mut environment = launched();
    environment[0] = (
        "COCKPIT_OPENCODE_BRIDGE",
        "/nonexistent/cockpit-opencode-bridge",
    );
    let events = fixtures(&["session-status-busy", "session-status-idle"]);

    let (received, stdout) = run_plugin(&events, PORT, &environment, 0);

    assert_eq!(received, Vec::<String>::new());
    assert_eq!(
        stdout.trim(),
        "hooks: experimental.chat.system.transform,event"
    );
}

/// Calls the plugin's system transform twice with `cockpit` as the `cockpit` program and
/// prints each resulting system prompt as JSON, one line each.
const SYSTEM_DRIVER: &str = r#"
const [plugin, port, apart] = process.argv.slice(2)
const { CockpitBridge } = await import(plugin)
const hooks = await CockpitBridge({ serverUrl: new URL(`http://127.0.0.1:${port}`) })
const now = Date.now
let elapsed = 0
Date.now = () => now() + elapsed
for (let call = 0; call < 3; call++) {
  if (call > 0) elapsed += Number(apart)
  const output = { system: ["OpenCode's own prompt"] }
  await hooks["experimental.chat.system.transform"]({ sessionID: "ses_1", model: {} }, output)
  console.log(JSON.stringify(output.system))
}
"#;

/// The system prompts three model calls `apart_ms` apart get, with a fake `cockpit` that runs
/// `brief_body` for `cockpit brief`, and how many times it ran.
fn system_prompts(brief_body: &str, apart_ms: u64) -> (Vec<String>, usize) {
    let dir = tempfile::tempdir().unwrap();
    let plugin = dir.path().join("cockpit-opencode-plugin.mjs");
    std::fs::write(&plugin, PLUGIN_SOURCE).unwrap();
    common::script(
        dir.path(),
        "cockpit",
        &format!(
            "[ \"$1\" = brief ] || exit 9\n\
             printf '%s' \"$COCKPIT_LAUNCH_TOKEN\" >> \"$(dirname \"$0\")/runs\"\n\
             echo >> \"$(dirname \"$0\")/runs\"\n{brief_body}"
        ),
    );
    let driver = dir.path().join("driver.mjs");
    std::fs::write(&driver, SYSTEM_DRIVER).unwrap();
    let output = Command::new("node")
        .arg(&driver)
        .arg(&plugin)
        .arg(PORT)
        .arg(apart_ms.to_string())
        .env(
            "COCKPIT_OPENCODE_BRIDGE",
            dir.path().join("cockpit-opencode-bridge"),
        )
        .env("COCKPIT_OPENCODE_COCKPIT", dir.path().join("cockpit"))
        .env("COCKPIT_OPENCODE_PORT", PORT)
        .env("COCKPIT_LAUNCH_TOKEN", "token-1")
        .output()
        .unwrap_or_else(|error| panic!("the plugin tests need Node.js on PATH: {error}"));
    assert!(output.status.success(), "{output:?}");
    let runs = std::fs::read_to_string(dir.path().join("runs"))
        .unwrap_or_default()
        .lines()
        .inspect(|token| assert_eq!(*token, "token-1"))
        .count();
    let prompts = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    (prompts, runs)
}

const OWN_PROMPT: &str = r#"["OpenCode's own prompt"]"#;

/// #419: the agent sees its launch brief without asking, in every model call, read once.
#[test]
fn every_system_prompt_carries_the_launch_brief_read_once() {
    let (prompts, runs) = system_prompts("printf '# Cockpit launch brief\\n'", 0);

    assert_eq!(
        prompts,
        [r##"["OpenCode's own prompt","# Cockpit launch brief"]"##; 3]
    );
    assert_eq!(runs, 1);
}

/// The hook runs before every model call of a turn: a daemon that is down must not make each
/// of them wait for `cockpit brief` again.
#[test]
fn a_brief_that_cannot_be_read_never_blocks_the_prompt_nor_is_asked_each_call() {
    let (prompts, runs) = system_prompts("echo 'no daemon' >&2; exit 1", 1_000);

    assert_eq!(prompts, [OWN_PROMPT; 3]);
    assert_eq!(runs, 1);
}

#[test]
fn a_brief_that_cannot_be_read_is_asked_again_a_minute_later() {
    let (prompts, runs) = system_prompts("exit 1", 61_000);

    assert_eq!(prompts, [OWN_PROMPT; 3]);
    assert_eq!(runs, 3);
}

#[test]
fn an_empty_brief_adds_nothing_and_is_read_once() {
    let (prompts, runs) = system_prompts("exit 0", 61_000);

    assert_eq!(prompts, [OWN_PROMPT; 3]);
    assert_eq!(runs, 1);
}

/// The plugin's list and the adapter's must not drift apart.
#[test]
fn the_plugin_forwards_the_types_the_adapter_normalizes() {
    let list = PLUGIN_SOURCE
        .split("const FORWARDED = [")
        .nth(1)
        .and_then(|rest| rest.split(']').next())
        .unwrap();
    let forwarded: Vec<&str> = list
        .split(',')
        .map(|item| item.trim().trim_matches('"'))
        .filter(|item| !item.is_empty())
        .collect();

    assert_eq!(forwarded, cockpit_opencode::events::FORWARDED_EVENTS);
}
