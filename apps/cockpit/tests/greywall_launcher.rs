//! The greywall launcher (config/bin/.local/bin/greywall-agents/claude): what it refuses before
//! any sandbox starts.

mod common;
use common::sandbox::*;

use std::fs;

#[test]
fn no_session_starts_in_the_live_dotfiles_checkout() {
    // Security audit C1: a session there could write code the host runs, unreviewed.
    let sandbox = Sandbox::new();
    let live = sandbox.home.join("dotfiles/apps");
    fs::create_dir_all(&live).unwrap();
    sandbox.shim("greywall", "");
    let output = sandbox.command(bin().join("greywall-agents/claude"), &[]).current_dir(&live).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("IdeaProjects/dotfiles"));
    assert!(sandbox.calls("greywall").is_empty());
}
