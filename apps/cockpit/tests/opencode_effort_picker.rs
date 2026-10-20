//! Exercise the configured shortcut in a real OpenCode session, not just at its home screen.
mod common;
use common::sandbox::*;

use std::fs;
use std::path::{Path, PathBuf};

struct OpenCode {
    terminal: common::Terminal,
}

impl Drop for OpenCode {
    fn drop(&mut self) {
        let _ = self.terminal.child.kill();
        let _ = self.terminal.child.wait();
    }
}

#[test]
fn the_configured_shortcut_opens_the_native_picker_inside_a_session() {
    let binary = std::env::var_os("OPENCODE_BINARY").map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".opencode/bin/opencode"));
    assert!(binary.is_file(), "this native integration test requires OpenCode V2 at {} (or OPENCODE_BINARY)", binary.display());
    let mut sandbox = Sandbox::new();
    let settings = fs::read_to_string(repo().join("config/opencode/.config/opencode/cli.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&settings).unwrap();
    let shortcut: &[u8] = match parsed["keybinds"]["variant.list"].as_str().unwrap() {
        "ctrl+alt+e" => b"\x1b\x05",
        "f6" => b"\x1b[17~",
        binding => panic!("add the terminal encoding for the configured shortcut: {binding}"),
    };
    for (name, folder) in [("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache")] {
        sandbox.env.insert(name.into(), sandbox.root.join(folder).display().to_string());
    }
    sandbox.env.insert("OPENCODE_CONFIG_CONTENT".into(), "{}".into());
    sandbox.env.insert("OPENCODE_DB".into(), sandbox.root.join("opencode.db").display().to_string());
    sandbox.env.insert("OPENCODE_CLI_CONFIG_CONTENT".into(), settings);
    sandbox.env.insert("OPENCODE_DISABLE_AUTOUPDATE".into(), "1".into());
    // Enable the bundled model catalog without real credentials; no prompt or model request is sent.
    sandbox.env.insert("OPENAI_API_KEY".into(), "diagnostic-placeholder-not-used-for-requests".into());
    let session = "ses_effortRegression123456789";
    let request = serde_json::json!({"id": session, "model": {"providerID": "openai", "id": "gpt-6.1-sol"},
        "location": {"directory": sandbox.root}}).to_string();
    let created = sandbox.command(&binary, &[]).current_dir(&sandbox.root)
        .args(["api", "--standalone", "post", "/api/session", "--data", &request]).output().unwrap();
    assert!(created.status.success(), "{}", stderr(&created));
    let environment: Vec<_> = sandbox.env.iter().map(|(name, value)| (name.as_str(), Path::new(value))).collect();
    let mut app = OpenCode { terminal: common::Terminal::start(&binary, &["--standalone", "--session", session], &environment) };
    app.terminal.pump(15.0, Some(b"OpenAI"));
    assert!(common::contains(&app.terminal.screen, b"OpenAI"), "OpenCode did not load the test model: {}", String::from_utf8_lossy(&app.terminal.screen));
    app.terminal.screen.clear();
    app.terminal.type_keys(shortcut);
    app.terminal.pump(3.0, Some(b"Select variant"));
    assert!(common::contains(&app.terminal.screen, b"Select variant"), "the configured shortcut did not open the native picker in the session");
}
