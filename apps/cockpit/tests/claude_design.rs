//! claude-design, the command a design session runs to link its card to a Claude Design canvas.

mod common;
use common::sandbox::*;

use serde_json::Value;
use std::fs;
use std::process::Output;

fn design(sandbox: &Sandbox, arguments: &[&str]) -> Output {
    let designs = sandbox.root.join("designs").display().to_string();
    sandbox.run(bin().join("claude-design"), arguments, &[("CLAUDE_DESIGNS_DIR", &designs), ("CLAUDE_CODE_SESSION_ID", SESSION_A)])
}

#[test]
fn a_canvas_is_linked_to_the_session_with_its_title() {
    let sandbox = Sandbox::new();
    let linking = design(&sandbox, &["link", "https://claude.ai/artifact/d31175a3-dc24-4166-aa81-2ccdf5efe83a", "Checkout  redesign"]);
    assert_eq!(linking.status.code(), Some(0));
    let link: Value = serde_json::from_str(&fs::read_to_string(sandbox.root.join(format!("designs/{SESSION_A}.json"))).unwrap()).unwrap();
    assert_eq!((link["url"].as_str(), link["title"].as_str()),
               (Some("https://claude.ai/artifact/d31175a3-dc24-4166-aa81-2ccdf5efe83a"), Some("Checkout redesign")));
}

#[test]
fn a_chat_address_becomes_the_canvas_address() {
    let sandbox = Sandbox::new();
    design(&sandbox, &["link", "https://claude.ai/chat/7efffff6-0466-41ab-83d1-3f1d403d357d?artifact=d31175a3-dc24-4166-aa81-2ccdf5efe83a", "Canvas"]);
    assert_eq!(stdout(&design(&sandbox, &["show"])).trim(), "https://claude.ai/artifact/d31175a3-dc24-4166-aa81-2ccdf5efe83a");
}

#[test]
fn other_addresses_and_missing_titles_are_refused() {
    let sandbox = Sandbox::new();
    for arguments in [["link", "https://evil.example/artifact/abcdefgh", "x"], ["link", "https://claude.ai/artifact/abcdefgh12", " "]] {
        assert_eq!(design(&sandbox, &arguments).status.code(), Some(2), "{arguments:?}");
    }
    assert!(!sandbox.root.join("designs").exists());
}

#[test]
fn unlink_removes_the_canvas_from_the_card() {
    let sandbox = Sandbox::new();
    design(&sandbox, &["link", "https://claude.ai/artifact/abcdefgh12", "Canvas"]);
    design(&sandbox, &["unlink"]);
    assert_eq!(stdout(&design(&sandbox, &["show"])).trim(), "No canvas linked to this session.");
}

#[test]
fn a_role_is_written_for_the_card_and_none_removes_it() {
    let sandbox = Sandbox::new();
    let role = sandbox.root.join(format!("designs/{SESSION_A}.role"));
    assert_eq!(design(&sandbox, &["role", "designer"]).status.code(), Some(0));
    assert_eq!(fs::read_to_string(&role).unwrap(), "designer");
    assert_eq!(design(&sandbox, &["role", "none"]).status.code(), Some(0));
    assert!(!role.exists());
}

#[test]
fn an_unknown_role_is_refused() {
    let sandbox = Sandbox::new();
    assert_eq!(design(&sandbox, &["role", "boss"]).status.code(), Some(2));
}
