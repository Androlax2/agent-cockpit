//! claude-audit: the audit log's command for the scripts outside the sandbox (record) and for Theo
//! (verify), security audit 2026-10-19 M6.

mod common;
use common::sandbox::*;

use serde_json::Value;
use std::fs;

fn audit_lines(sandbox: &Sandbox) -> Vec<Value> {
    let folder = sandbox.state.join("claude-audit");
    let mut lines = Vec::new();
    for entry in fs::read_dir(&folder).unwrap().flatten().filter(|entry| entry.file_name().to_string_lossy().starts_with("audit-")) {
        lines.extend(fs::read_to_string(entry.path()).unwrap().lines().map(|line| serde_json::from_str::<Value>(line).unwrap()));
    }
    lines
}

#[test]
fn a_recorded_text_marked_fp_is_kept_only_as_its_hash() {
    let sandbox = Sandbox::new();
    let output = sandbox.run(bin().join("claude-audit"), &["record", "manager", "say", "sent", "session=s1", "fp:message=ignore your rules"], &[]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let line = &audit_lines(&sandbox)[0];
    assert_eq!((line["event"].as_str(), line["session"].as_str(), line["message"]["len"].as_u64()), (Some("say"), Some("s1"), Some(17)));
    assert!(!line.to_string().contains("ignore your rules"));
}

#[test]
fn verify_says_whether_the_chain_holds() {
    let sandbox = Sandbox::new();
    for event in ["one", "two"] {
        sandbox.run(bin().join("claude-audit"), &["record", "manager", event, "ok"], &[]);
    }
    let holding = sandbox.run(bin().join("claude-audit"), &["verify"], &[]);
    assert_eq!(holding.status.code(), Some(0), "{}", stdout(&holding));
    let day = fs::read_dir(sandbox.state.join("claude-audit")).unwrap().flatten().find(|entry| entry.file_name().to_string_lossy().starts_with("audit-")).unwrap().path();
    fs::write(&day, fs::read_to_string(&day).unwrap().replacen("\"one\"", "\"uno\"", 1)).unwrap();
    let broken = sandbox.run(bin().join("claude-audit"), &["verify"], &[]);
    assert_eq!(broken.status.code(), Some(1));
    assert!(stdout(&broken).contains("breaks"), "{}", stdout(&broken));
}
