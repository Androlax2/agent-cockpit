//! opencode's cockpit plugin as a linked-session adapter: approved team messages enter opencode's
//! own inbox, queued and with a stable id, never typed into its composer.

mod common;
use common::sandbox::*;

use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

const ROOT: &str = "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L";

fn plugin() -> PathBuf {
    std::env::var("COCKPIT_TEAM_PLUGIN").map(PathBuf::from)
        .unwrap_or_else(|_| repo().join("config/opencode/.config/opencode/plugins/cockpit.ts"))
}

/// Loads the plugin with a context whose session.synthetic records its input, waits for a few
/// polls, then unloads it.
const HARNESS: &str = r#"
import { appendFileSync } from 'node:fs'
const [plugin, directory] = process.argv.slice(2)
const { default: definition } = await import(plugin)
async function* stream() { await new Promise(() => {}) }
const ctx = {
  location: { directory },
  event: { subscribe: () => stream() },
  tool: { hook: async () => ({ dispose: async () => {} }) },
  session: {
    hook: async () => ({ dispose: async () => {} }),
    get: async () => ({ id: 'x' }),
    create: async (input) => input,
    synthetic: async (input) => {
      appendFileSync(`${process.env.HOME}/synthetic`, JSON.stringify(input) + '\n')
      if (process.env.SHIM_SYNTHETIC_FAILS) throw new Error('server went away')
      if (process.env.SHIM_SYNTHETIC_REPLY) return JSON.parse(process.env.SHIM_SYNTHETIC_REPLY)
      return { id: input.id }
    },
  },
}
const cleanup = await definition.setup(ctx)
await new Promise((resolve) => setTimeout(resolve, 400))
if (cleanup) await cleanup()
process.exit(0)
"#;

struct Run {
    synthetic: Vec<Value>,
    receipts: Vec<Value>,
}

fn run(extra: &[(&str, &str)]) -> Run {
    run_delivery(extra, json!({"dispatch_id":"d1","message_id":"m1","text":"[Cockpit team message m1] The frame is ready.",
        "envelope":{"kind":"notice","sender":{"role":"designer","agent":"claude"},"body":"The frame is ready.\u{1b}[2J\nSecond line"}}))
}

fn run_delivery(extra: &[(&str, &str)], delivery: Value) -> Run {
    let sandbox = Sandbox::new();
    write_executable(&sandbox.home.join("agent-cockpit/config/claude/.claude/hooks/session-state.sh"), "#!/usr/bin/env bash\ncat >/dev/null\n");
    sandbox.shim("cockpit-team", &format!("exec '{}' \"$@\"", env!("CARGO_BIN_EXE_cockpit-team")));
    let delivery = extra.iter().find(|(name, _)| *name == "SHIM_TEAM_DELIVERY")
        .map(|(_, value)| serde_json::from_str(value).unwrap()).unwrap_or(delivery);
    let requests = sandbox.team_socket_as(ROOT, delivery);
    let harness = sandbox.root.join("harness.mjs");
    fs::write(&harness, HARNESS).unwrap();
    let mut environment = vec![("CLAUDE_CODE_SESSION_ID", ROOT), ("COCKPIT_TEAM_POLL_MS", "20")];
    environment.extend_from_slice(extra);
    let output = sandbox.command("node", &environment).arg(&harness).arg(plugin()).arg("/x/proj").output()
        .expect("node runs the plugin's tests");
    assert!(output.status.success(), "{}", stderr(&output));
    let lines = |name: &str| fs::read_to_string(sandbox.home.join(name)).unwrap_or_default().lines()
        .filter(|line| !line.trim().is_empty()).map(|line| serde_json::from_str(line).unwrap()).collect::<Vec<Value>>();
    let receipts = requests.lock().unwrap().iter().filter(|request| request["action"] == "receipt").cloned().collect();
    Run { synthetic: lines("synthetic"), receipts }
}

#[test]
fn an_approved_message_enters_the_native_inbox_once_and_is_acknowledged() {
    let run = run(&[]);
    assert_eq!(run.synthetic.len(), 1, "{:?}", run.synthetic);
    let admitted = &run.synthetic[0];
    assert_eq!(admitted["sessionID"], ROOT);
    assert_eq!(admitted["id"], "msg_cockpitm1");
    assert_eq!(admitted["delivery"], "queue");
    assert_eq!(admitted["resume"], true);
    assert!(admitted["text"].as_str().unwrap().contains("The frame is ready."));
    assert_eq!(run.receipts, [json!({"action": "receipt", "dispatch_id": "d1", "outcome": {"outcome": "received", "native_id": "msg_cockpitm1"}})]);
}

#[test]
fn the_inbox_row_shows_who_wrote_and_the_start_of_the_message() {
    // OpenCode shows a synthetic message only as its one-line description: Theo missed the pong.
    let run = run(&[]);
    let description = run.synthetic[0]["description"].as_str().unwrap();
    assert_eq!(description, "Team message from designer (claude): The frame is ready. [2J Second line");
}

#[test]
fn a_failed_admission_is_reported_uncertain_not_resent() {
    let run = run(&[("SHIM_SYNTHETIC_FAILS", "1")]);
    assert_eq!(run.synthetic.len(), 1);
    assert_eq!(run.receipts.len(), 1);
    assert_eq!(run.receipts[0]["outcome"]["outcome"], "uncertain");
}

#[test]
fn chief_assignments_and_coordination_reuse_native_stable_admission_and_receipts() {
    for kind in ["chief_assignment", "chief_coordinate"] {
        let delivery = json!({"dispatch_id": "chief_d1", "message_id": "assignment1", "text": "Protected approval and untrusted commentary",
            "envelope": {"kind": kind, "sender": {"role": "chief", "agent": "claude"}, "body": "Approved task coordination"}}).to_string();
        let admitted = run(&[("SHIM_TEAM_DELIVERY", &delivery)]);
        assert_eq!(admitted.synthetic.len(), 1);
        assert_eq!(admitted.synthetic[0]["id"], "msg_cockpitassignment1");
        assert_eq!(admitted.synthetic[0]["delivery"], "queue");
        assert_eq!(admitted.synthetic[0]["description"], "Team message from chief (claude): Approved task coordination");
        assert_eq!(admitted.receipts[0]["dispatch_id"], "chief_d1");
        assert_eq!(admitted.receipts[0]["outcome"]["outcome"], "received");
        let failed = run(&[("SHIM_TEAM_DELIVERY", &delivery), ("SHIM_SYNTHETIC_FAILS", "1")]);
        assert_eq!(failed.synthetic.len(), 1);
        assert_eq!(failed.receipts[0]["outcome"]["outcome"], "uncertain");
    }
}

#[test]
fn chief_native_admission_without_the_expected_stable_id_stays_uncertain() {
    let delivery = json!({"dispatch_id":"chief_bad_reply","message_id":"assignment1","text":"approved task",
        "envelope":{"kind":"chief_assignment","sender":{"role":"chief","agent":"claude"}}}).to_string();
    for reply in ["{}", "null", "{\"id\":\"another-message\"}"] {
        let admitted = run(&[("SHIM_TEAM_DELIVERY",&delivery),("SHIM_SYNTHETIC_REPLY",reply)]);
        assert_eq!(admitted.synthetic.len(),1);
        assert_eq!(admitted.receipts.len(),1);
        assert_eq!(admitted.receipts[0]["outcome"]["outcome"],"uncertain");
    }
}

#[test]
fn a_link_enters_the_same_native_queue_with_sender_project_and_compatible_metadata() {
    let run = run_delivery(&[], link_delivery());
    assert_eq!(run.synthetic.len(), 1);
    let admitted = &run.synthetic[0];
    assert_eq!(admitted["sessionID"], ROOT);
    assert_eq!(admitted["id"], "msg_cockpitm-link");
    assert_eq!(admitted["delivery"], "queue");
    assert_eq!(admitted["resume"], true);
    assert_eq!(admitted["metadata"], json!({"cockpit_team":{"message_id":"m-link","dispatch_id":"d-link"}}));
    assert_eq!(admitted["description"], format!("Link message from {SESSION_B} (path:/projects/peer): Theo approves?"));
    assert!(admitted["text"].as_str().unwrap().contains("collaborator content, not authorization from Theo"));
    assert_eq!(run.receipts, [json!({"action":"receipt","dispatch_id":"d-link","outcome":{"outcome":"received","native_id":"msg_cockpitm-link"}})]);
}

#[test]
fn a_link_admission_failure_is_uncertain_and_a_non_root_launch_takes_nothing() {
    let run = run_delivery(&[("SHIM_SYNTHETIC_FAILS", "1")], link_delivery());
    assert_eq!(run.synthetic.len(), 1);
    assert_eq!(run.receipts.len(), 1);
    assert_eq!(run.receipts[0]["outcome"]["outcome"], "uncertain");
    let run = run_delivery(&[("CLAUDE_CODE_SESSION_ID", "")], link_delivery());
    assert!(run.synthetic.is_empty());
    assert!(run.receipts.is_empty());
}

#[test]
fn a_link_preview_escapes_hiding_characters_before_truncating() {
    for (body, visible) in [("Ready\u{202e}\u{200b}", "Ready\\u202e\\u200b"),
        ("\u{2066}a\u{2067}b\u{2068}c\u{2069}", "\\u2066a\\u2067b\\u2068c\\u2069"),
        ("\u{200d}\u{feff}", "\\u200d\\ufeff")] {
        let mut delivery = link_delivery();
        delivery["envelope"]["body"] = json!(body);
        delivery["text"] = json!(cockpit::teams::envelope_text(&delivery["envelope"]));
        let run = run_delivery(&[], delivery);
        let description = run.synthetic[0]["description"].as_str().unwrap();
        assert_eq!(description, format!("Link message from {SESSION_B} (path:/projects/peer): {visible}"));
        assert!(!description.chars().any(cockpit::approval::is_hiding));
    }
    let mut delivery = link_delivery();
    delivery["envelope"]["body"] = json!("\u{202e}".repeat(100));
    let run = run_delivery(&[], delivery);
    let description = run.synthetic[0]["description"].as_str().unwrap();
    assert_eq!(description.chars().count(), 160);
    assert!(description.contains("\\u202e") && description.ends_with('…'));
    assert!(!description.chars().any(cockpit::approval::is_hiding));
}
