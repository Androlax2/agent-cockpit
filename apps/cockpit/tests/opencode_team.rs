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
    get: async () => ({ id: 'x' }),
    create: async (input) => input,
    synthetic: async (input) => {
      appendFileSync(`${process.env.HOME}/synthetic`, JSON.stringify(input) + '\n')
      if (process.env.SHIM_SYNTHETIC_FAILS) throw new Error('server went away')
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
    let sandbox = Sandbox::new();
    write_executable(&sandbox.home.join("dotfiles/config/claude/.claude/hooks/session-state.sh"), "#!/usr/bin/env bash\ncat >/dev/null\n");
    sandbox.shim("cockpit-team", r#"case $1 in
  receive) if [ ! -f "$HOME/delivered" ]; then touch "$HOME/delivered"
             echo '{"dispatch_id":"d1","message_id":"m1","text":"[Cockpit team message m1] The frame is ready."}'
           else echo null; fi ;;
  receipt) cat >> "$HOME/receipts"; echo >> "$HOME/receipts"; echo '{"status":"recorded"}' ;;
esac"#);
    let harness = sandbox.root.join("harness.mjs");
    fs::write(&harness, HARNESS).unwrap();
    let mut environment = vec![("CLAUDE_CODE_SESSION_ID", ROOT), ("COCKPIT_TEAM_POLL_MS", "20")];
    environment.extend_from_slice(extra);
    let output = sandbox.command("node", &environment).arg(&harness).arg(plugin()).arg("/x/proj").output()
        .expect("node runs the plugin's tests");
    assert!(output.status.success(), "{}", stderr(&output));
    let lines = |name: &str| fs::read_to_string(sandbox.home.join(name)).unwrap_or_default().lines()
        .filter(|line| !line.trim().is_empty()).map(|line| serde_json::from_str(line).unwrap()).collect::<Vec<Value>>();
    Run { synthetic: lines("synthetic"), receipts: lines("receipts") }
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
fn a_failed_admission_is_reported_uncertain_not_resent() {
    let run = run(&[("SHIM_SYNTHETIC_FAILS", "1")]);
    assert_eq!(run.synthetic.len(), 1);
    assert_eq!(run.receipts.len(), 1);
    assert_eq!(run.receipts[0]["outcome"]["outcome"], "uncertain");
}
