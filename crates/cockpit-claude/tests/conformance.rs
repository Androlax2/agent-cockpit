//! Conformance against an installed Claude Code. Ignored by default because CI has no
//! Claude Code; run it with the binary of `CONFORMANCE_VERSION`:
//!
//! ```text
//! COCKPIT_CLAUDE_PROGRAM=~/.local/share/claude/versions/2.1.316 \
//!   cargo nextest run -p cockpit-claude --test conformance --run-ignored only
//! ```

mod common;

use std::path::PathBuf;
use std::process::Command;

use cockpit_claude::{CONFORMANCE_VERSION, ClaudeCode};
use cockpit_provider_api::Provider;

fn installed() -> PathBuf {
    PathBuf::from(
        std::env::var_os("COCKPIT_CLAUDE_PROGRAM")
            .expect("COCKPIT_CLAUDE_PROGRAM names the Claude Code binary to test"),
    )
}

#[test]
#[ignore = "needs an installed Claude Code (COCKPIT_CLAUDE_PROGRAM)"]
fn the_installed_claude_code_is_the_pinned_version() {
    let claude = ClaudeCode::discover(&installed(), Some(common::bridge())).unwrap();

    assert_eq!(claude.version(), Some(CONFORMANCE_VERSION));
}

#[test]
#[ignore = "needs an installed Claude Code (COCKPIT_CLAUDE_PROGRAM)"]
fn the_installed_claude_code_documents_every_flag_the_adapter_passes() {
    let output = Command::new(installed()).arg("--help").output().unwrap();
    let help = String::from_utf8_lossy(&output.stdout);

    for flag in cockpit_claude::LAUNCH_FLAGS {
        assert!(help.contains(flag), "{flag} is not in --help");
    }
    for level in cockpit_claude::EFFORT_LEVELS {
        assert!(help.contains(level), "effort {level} is not in --help");
    }
}

/// An environment variable the binary no longer reads would silently stop protecting the
/// launch; `CLAUDE_CODE_DISABLE_AGENT_VIEW` removing `/background` was measured by hand
/// on 2.1.316 (docs/decisions/2026-10-31-184-claude-launch-without-background-daemon.md).
#[test]
#[ignore = "needs an installed Claude Code (COCKPIT_CLAUDE_PROGRAM)"]
fn the_installed_claude_code_reads_every_variable_the_adapter_sets() {
    let binary = std::fs::read(installed()).unwrap();

    for (name, _) in cockpit_claude::LAUNCH_ENVIRONMENT {
        assert!(
            binary
                .windows(name.len())
                .any(|window| window == name.as_bytes()),
            "{name} is not in the binary"
        );
    }
}
