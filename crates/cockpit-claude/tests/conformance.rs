//! Conformance against an installed Claude Code. Ignored by default because CI has no
//! Claude Code; run it with the Claude Code to check (the nightly job runs the latest, #732):
//!
//! ```text
//! COCKPIT_CLAUDE_PROGRAM=~/.local/share/claude/versions/2.1.316 \
//!   cargo nextest run -p cockpit-claude --test conformance --run-ignored only
//! ```

mod common;

use std::process::Command;

use cockpit_claude::ClaudeCode;
use cockpit_provider_api::Provider;

#[test]
#[ignore = "needs an installed Claude Code (COCKPIT_CLAUDE_PROGRAM)"]
fn the_installed_claude_code_prints_a_version_the_adapter_reads() {
    let claude = ClaudeCode::discover(&common::installed(), Some(common::bridge())).unwrap();

    assert!(claude.version().is_some());
}

#[test]
#[ignore = "needs an installed Claude Code (COCKPIT_CLAUDE_PROGRAM)"]
fn the_installed_claude_code_documents_every_flag_the_adapter_passes() {
    let output = Command::new(common::installed())
        .arg("--help")
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&output.stdout);

    for flag in cockpit_claude::LAUNCH_FLAGS {
        assert!(help.contains(flag), "{flag} is not in --help");
    }
}

/// #743: Torlek reads the levels from `--help`; a rewording that hides them would silently
/// turn the check into a pass-through.
#[test]
#[ignore = "needs an installed Claude Code (COCKPIT_CLAUDE_PROGRAM)"]
fn the_installed_claude_code_lists_its_effort_levels_where_the_adapter_reads_them() {
    let claude = ClaudeCode::discover(&common::installed(), None).unwrap();

    let levels = claude.effort_levels();

    assert!(!levels.is_empty(), "no effort levels in `claude --help`");
    assert!(claude.check_choices(None, Some(&levels[0])).is_ok());
    assert!(claude.check_choices(None, Some("no-such-level")).is_err());
}

/// A variable the binary no longer reads would silently stop protecting the launch; what
/// each one does was measured by hand (`LAUNCH_ENVIRONMENT`, #184).
#[test]
#[ignore = "needs an installed Claude Code (COCKPIT_CLAUDE_PROGRAM)"]
fn the_installed_claude_code_reads_every_variable_the_adapter_sets() {
    let binary = std::fs::read(common::installed()).unwrap();

    for (name, _) in cockpit_claude::LAUNCH_ENVIRONMENT {
        assert!(
            binary
                .windows(name.len())
                .any(|window| window == name.as_bytes()),
            "{name} is not in the binary"
        );
    }
}

/// The trust record a parallel clone's launch writes (#702) is the one this binary reads and
/// locks: its key, its own remedy naming that key, and the `<file>.lock` its saves take.
#[test]
#[ignore = "needs an installed Claude Code (COCKPIT_CLAUDE_PROGRAM)"]
fn the_installed_claude_code_keeps_folder_trust_where_the_adapter_records_it() {
    let binary = std::fs::read(common::installed()).unwrap();
    let contains = |text: &str| {
        binary
            .windows(text.len())
            .any(|window| window == text.as_bytes())
    };

    for text in [
        "?.hasTrustDialogAccepted===!0",
        "].hasTrustDialogAccepted: true in ",
        ".lock`",
    ] {
        assert!(contains(text), "{text:?} is not in the binary");
    }
}
