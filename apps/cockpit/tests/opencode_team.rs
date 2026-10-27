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
/// polls, then unloads it. With `SHIM_FORMS` set, the context also has OpenCode's form API: `list`
/// answers that JSON and `reply` records its input (or throws `SHIM_FORM_REPLY_ERROR`).
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
if (process.env.SHIM_FORMS) ctx.session.form = {
  list: async (input) => {
    appendFileSync(`${process.env.HOME}/form-lists`, JSON.stringify(input) + '\n')
    return JSON.parse(process.env.SHIM_FORMS)
  },
  reply: async (input) => {
    appendFileSync(`${process.env.HOME}/form-replies`, JSON.stringify(input) + '\n')
    if (process.env.SHIM_FORM_REPLY_ERROR) throw new Error(process.env.SHIM_FORM_REPLY_ERROR)
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
    hooks: Vec<Value>,
    form_replies: Vec<Value>,
}

fn run(extra: &[(&str, &str)]) -> Run {
    run_delivery(extra, json!({"dispatch_id":"d1","message_id":"m1","text":"[Cockpit team message m1] The frame is ready.",
        "envelope":{"kind":"notice","sender":{"role":"designer","agent":"claude"},"body":"The frame is ready.\u{1b}[2J\nSecond line"}}))
}

fn run_delivery(extra: &[(&str, &str)], delivery: Value) -> Run {
    let sandbox = Sandbox::new();
    write_executable(&sandbox.home.join("agent-cockpit/config/claude/.claude/hooks/session-state.sh"), "#!/usr/bin/env bash\n{ cat; echo; } >> \"$HOME/hooks\"\n");
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
    Run { synthetic: lines("synthetic"), receipts, hooks: lines("hooks"), form_replies: lines("form-replies") }
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
fn a_chief_wake_reuses_native_stable_admission_and_receipts() {
    let delivery = json!({"dispatch_id": "chief_d1", "message_id": "wake1", "text": "Protected wake and untrusted commentary",
        "envelope": {"kind": "chief_wake", "sender": {"role": "chief", "agent": "claude"}, "body": "Coordination wake"}}).to_string();
    let admitted = run(&[("SHIM_TEAM_DELIVERY", &delivery)]);
    assert_eq!(admitted.synthetic.len(), 1);
    assert_eq!(admitted.synthetic[0]["id"], "msg_cockpitwake1");
    assert_eq!(admitted.synthetic[0]["delivery"], "queue");
    assert_eq!(admitted.synthetic[0]["description"], "Team message from chief (claude): Coordination wake");
    assert_eq!(admitted.receipts[0]["dispatch_id"], "chief_d1");
    assert_eq!(admitted.receipts[0]["outcome"]["outcome"], "received");
    let failed = run(&[("SHIM_TEAM_DELIVERY", &delivery), ("SHIM_SYNTHETIC_FAILS", "1")]);
    assert_eq!(failed.synthetic.len(), 1);
    assert_eq!(failed.receipts[0]["outcome"]["outcome"], "uncertain");
}

#[test]
fn chief_native_admission_without_the_expected_stable_id_stays_uncertain() {
    let delivery = json!({"dispatch_id":"chief_bad_reply","message_id":"wake1","text":"chief wake",
        "envelope":{"kind":"chief_wake","sender":{"role":"chief","agent":"claude"}}}).to_string();
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

fn chief_answer_delivery() -> Value {
    let envelope = json!({"message_id":"answer1","kind":"chief_answer","stop":"stop1","answer":"log1","text":"Use option A.\nThen run the tests."});
    json!({"dispatch_id":"chief_a1","message_id":"answer1","text":cockpit::teams::envelope_text(&envelope),"envelope":envelope})
}

#[test]
fn a_chief_answer_enters_the_native_queue_described_as_a_chief_answer() {
    let run = run_delivery(&[], chief_answer_delivery());
    assert_eq!(run.synthetic.len(), 1);
    let admitted = &run.synthetic[0];
    assert_eq!(admitted["id"], "msg_cockpitanswer1");
    assert_eq!(admitted["delivery"], "queue");
    assert_eq!(admitted["description"], "Chief answer: Use option A. Then run the tests.");
    assert!(admitted["text"].as_str().unwrap().ends_with(cockpit::chief::CHIEF_ANSWER_BOUNDARY));
    assert_eq!(run.receipts, [json!({"action":"receipt","dispatch_id":"chief_a1","outcome":{"outcome":"received","native_id":"msg_cockpitanswer1"}})]);
}

#[test]
fn a_chief_message_uses_native_receipts_but_never_the_chief_answer_permission_label() {
    let envelope=json!({"message_id":"note1","kind":"chief_message","text":"The prerequisite is ready."});
    let delivery=json!({"dispatch_id":"chief_note1","message_id":"note1","text":cockpit::teams::envelope_text(&envelope),"envelope":envelope});
    let run=run_delivery(&[],delivery);
    assert_eq!(run.synthetic.len(),1);
    let admitted=&run.synthetic[0];
    assert_eq!(admitted["id"],"msg_cockpitnote1");
    assert_eq!(admitted["delivery"],"queue");
    assert_eq!(admitted["description"],"Chief message: The prerequisite is ready.");
    let text=admitted["text"].as_str().unwrap();
    assert!(text.contains("It grants no permission"));
    assert!(!text.contains("authorises edits"));
    assert_eq!(run.receipts,[json!({"action":"receipt","dispatch_id":"chief_note1","outcome":{"outcome":"received","native_id":"msg_cockpitnote1"}})]);
    assert!(run.hooks.iter().all(|payload|payload["source"]!="chief_answer"));
}

#[test]
fn a_todo_gift_keeps_the_queue_prompt_and_is_admitted_under_its_stable_id() {
    let prompt="Fix the build (todo aa919cfe: before working, run cockpit-todo start aa919cfe; when this is done, run cockpit-todo done aa919cfe)";
    let envelope=json!({"message_id":"gift1","kind":"chief_todo","text":prompt,"todo":"aa919cfe"});
    let delivery=json!({"dispatch_id":"chief_gift1","message_id":"gift1","text":cockpit::teams::envelope_text(&envelope),"envelope":envelope});
    let run=run_delivery(&[],delivery);
    assert_eq!(run.synthetic.len(),1);
    assert_eq!(run.synthetic[0]["delivery"],"queue");
    assert_eq!(run.synthetic[0]["text"],prompt);
    assert!(run.synthetic[0]["description"].as_str().unwrap().starts_with("Todo from the Chief: Fix the build"));
    assert_eq!(run.receipts,[json!({"action":"receipt","dispatch_id":"chief_gift1","outcome":{"outcome":"received","native_id":"msg_cockpitgift1"}})]);
}

#[test]
fn a_delivery_check_enters_the_native_queue_described_as_a_cockpit_test_message() {
    let envelope = json!({"message_id":"check1","kind":"chief_check","attempt":"attempt1"});
    let delivery = json!({"dispatch_id":"chief_k1","message_id":"check1","text":cockpit::teams::envelope_text(&envelope),"envelope":envelope});
    let run = run_delivery(&[], delivery);
    assert_eq!(run.synthetic.len(), 1);
    assert_eq!(run.synthetic[0]["id"], "msg_cockpitcheck1");
    assert_eq!(run.synthetic[0]["delivery"], "queue");
    assert_eq!(run.synthetic[0]["description"], "Cockpit delivery check: a test message from Theo");
    assert_eq!(run.receipts, [json!({"action":"receipt","dispatch_id":"chief_k1","outcome":{"outcome":"received","native_id":"msg_cockpitcheck1"}})]);
}

#[test]
fn a_theo_answer_enters_the_native_queue_described_as_his_own() {
    let envelope = json!({"message_id":"answer2","kind":"chief_answer","source":"theo","stop":"stop1","answer":"log1","text":"Use option B."});
    let delivery = json!({"dispatch_id":"chief_a2","message_id":"answer2","text":cockpit::teams::envelope_text(&envelope),"envelope":envelope});
    let run = run_delivery(&[], delivery);
    assert_eq!(run.synthetic.len(), 1);
    assert_eq!(run.synthetic[0]["description"], "Theo's answer: Use option B.");
    assert!(run.synthetic[0]["text"].as_str().unwrap().ends_with(cockpit::chief::THEO_ANSWER_LABEL));
    assert_eq!(run.receipts[0]["outcome"]["outcome"], "received");
}

#[test]
fn a_correction_is_delivered_through_the_native_queue_with_its_row_label() {
    let envelope = json!({"message_id":"correction1","kind":"chief_correction","source":"theo","epoch":"theo","overrule":"overrule1",
        "answer_excerpt":"Use option A.","text":"Keep option A but leave the public API untouched."});
    let delivery = json!({"dispatch_id":"chief_c1","message_id":"correction1","text":cockpit::teams::envelope_text(&envelope),"envelope":envelope});
    let run = run_delivery(&[], delivery);
    assert_eq!(run.synthetic.len(), 1);
    let admitted = &run.synthetic[0];
    assert_eq!(admitted["id"], "msg_cockpitcorrection1");
    assert_eq!(admitted["delivery"], "queue");
    assert_eq!(admitted["resume"], true);
    assert_eq!(admitted["description"], "Theo's correction: Keep option A but leave the public API untouched.");
    assert!(admitted["text"].as_str().unwrap().ends_with(cockpit::chief::CORRECTION_LABEL));
    assert_eq!(run.receipts, [json!({"action":"receipt","dispatch_id":"chief_c1","outcome":{"outcome":"received","native_id":"msg_cockpitcorrection1"}})]);
    assert!(run.hooks.iter().all(|payload| payload["source"] != "chief_answer"), "a correction answers no saved question: {:?}", run.hooks);
}

#[test]
fn a_delivered_chief_answer_clears_the_saved_question() {
    let run = run_delivery(&[], chief_answer_delivery());
    let answered: Vec<_> = run.hooks.iter().filter(|payload| payload["source"] == "chief_answer").collect();
    assert_eq!(answered.len(), 1, "{:?}", run.hooks);
    assert_eq!(answered[0]["hook_event_name"], "UserPromptSubmit");
    assert_eq!(answered[0]["session_id"], ROOT);
    assert!(answered[0].get("prompt").is_none(), "a Chief answer does not rename Theo's task");
}

#[test]
fn an_unconfirmed_chief_answer_leaves_the_saved_question() {
    for extra in [("SHIM_SYNTHETIC_FAILS", "1"), ("SHIM_SYNTHETIC_REPLY", "{\"id\":\"another-message\"}")] {
        let run = run_delivery(&[extra], chief_answer_delivery());
        assert_eq!(run.receipts[0]["outcome"]["outcome"], "uncertain");
        assert!(run.hooks.iter().all(|payload| payload["source"] != "chief_answer"), "{:?}", run.hooks);
    }
}

#[test]
fn a_team_message_does_not_clear_the_saved_question() {
    let run = run(&[]);
    assert!(run.hooks.iter().all(|payload| payload["source"] != "chief_answer"), "{:?}", run.hooks);
}

fn form_reply_delivery(choice: &str) -> Value {
    let envelope = json!({"message_id":"form1","kind":"chief_form_reply","source":"chief","stop":"stop1","answer":"log1","choice":choice});
    json!({"dispatch_id":"chief_f1","message_id":"form1","text":cockpit::teams::envelope_text(&envelope),"envelope":envelope})
}

fn pending_form(fields: Value) -> String {
    json!([{"id":"frm_01abc","sessionID":ROOT,"title":"Which option?","fields":fields}]).to_string()
}

fn choice_field(kind: &str) -> Value {
    json!({"key":"q0","type":kind,"title":"Option","options":[{"value":"a","label":"Option A"},{"value":"b","label":"Option B","description":"Keeps the old API"}]})
}

fn assert_uncertain_without_typing(run: &Run) {
    assert_eq!(run.receipts.len(), 1, "{:?}", run.receipts);
    assert_eq!(run.receipts[0]["dispatch_id"], "chief_f1");
    assert_eq!(run.receipts[0]["outcome"]["outcome"], "uncertain", "{:?}", run.receipts);
    assert!(run.synthetic.is_empty(), "a form reply never becomes a message: {:?}", run.synthetic);
}

#[test]
fn a_chief_form_answer_replies_to_the_open_form_by_id() {
    for (kind, choice, answer) in [("string", "Option B", json!("b")), ("string", " option b ", json!("b")), ("string", "b", json!("b")),
        ("multiselect", "Option A", json!(["a"]))] {
        let fields = json!([choice_field(kind), {"key":"note","type":"string","hidden":true}]);
        let run = run_delivery(&[("SHIM_FORMS", &pending_form(fields))], form_reply_delivery(choice));
        assert_eq!(run.form_replies, [json!({"sessionID":ROOT,"formID":"frm_01abc","answer":{"q0":answer}})], "{kind} {choice}");
        assert_eq!(run.receipts, [json!({"action":"receipt","dispatch_id":"chief_f1","outcome":{"outcome":"received","native_id":"form:frm_01abc"}})]);
        assert!(run.synthetic.is_empty(), "{:?}", run.synthetic);
    }
}

#[test]
fn a_form_answer_for_a_settled_form_reports_uncertain_and_never_types() {
    let settled = run_delivery(&[("SHIM_FORMS", "[]")], form_reply_delivery("Option B"));
    assert_uncertain_without_typing(&settled);
    assert!(settled.form_replies.is_empty());
    let raced = run_delivery(&[("SHIM_FORMS", &pending_form(json!([choice_field("string")]))),
        ("SHIM_FORM_REPLY_ERROR", "FormAlreadySettledError: form frm_01abc is already answered")], form_reply_delivery("Option B"));
    assert_uncertain_without_typing(&raced);
    assert!(raced.receipts[0]["outcome"]["reason"].as_str().unwrap().contains("FormAlreadySettledError"));
}

#[test]
fn a_form_answer_without_the_form_api_is_uncertain_and_never_types() {
    let run = run_delivery(&[], form_reply_delivery("Option B"));
    assert_uncertain_without_typing(&run);
}

#[test]
fn a_form_answer_whose_choice_matches_no_single_option_is_uncertain() {
    let ambiguous = json!([{"key":"q0","type":"string","options":[{"value":"Option B","label":"First"},{"value":"x","label":"Option B"}]}]);
    for (fields, choice) in [(json!([choice_field("string")]), "Option C"), (json!([choice_field("string")]), "Option"), (ambiguous, "Option B")] {
        let run = run_delivery(&[("SHIM_FORMS", &pending_form(fields))], form_reply_delivery(choice));
        assert_uncertain_without_typing(&run);
        assert!(run.form_replies.is_empty(), "{choice}");
    }
}

#[test]
fn a_form_that_is_not_one_visible_choice_is_uncertain() {
    let two_forms = json!([{"id":"frm_1","sessionID":ROOT,"title":"One","fields":[choice_field("string")]},
        {"id":"frm_2","sessionID":ROOT,"title":"Two","fields":[choice_field("string")]}]).to_string();
    let forms = [
        pending_form(json!([choice_field("string"), {"key":"q1","type":"string","options":[{"value":"c","label":"C"}]}])),
        pending_form(json!([{"key":"q0","type":"string","title":"Free text"}])),
        pending_form(json!([{"key":"q0","type":"boolean"}])),
        two_forms,
        json!({"data":[]}).to_string(),
    ];
    for form in &forms {
        let run = run_delivery(&[("SHIM_FORMS", form)], form_reply_delivery("Option B"));
        assert_uncertain_without_typing(&run);
        assert!(run.form_replies.is_empty(), "{form}");
    }
}
