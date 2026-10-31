//! The hook bridge against a live Claude Code (#184): the payloads it writes and the launch
//! token its hooks inherit. The live test is ignored by default because CI has no Claude
//! Code and it spends one short model turn; run it with the binary of `CONFORMANCE_VERSION`:
//!
//! ```text
//! COCKPIT_CLAUDE_PROGRAM=~/.local/share/claude/versions/2.1.316 \
//!   cargo nextest run -p cockpit-claude --test live_hooks --run-ignored only
//! ```

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cockpit_claude::hooks;
use cockpit_claude::{Bridge, ClaudeCode};
use cockpit_provider_api::{Conversation, ConversationId, EventKind, LaunchRequest, Provider};

#[test]
fn a_resumed_session_start_names_the_resumed_conversation() {
    let event = hooks::normalize(&common::fixture("session-start-resume")).unwrap();

    assert_eq!(
        event.kind,
        EventKind::Started {
            conversation: Some(
                ConversationId::new("576d7a7d-768a-47d6-ae5c-290c19e81fdc").unwrap()
            )
        }
    );
}

/// A `cockpit` that appends its launch token and arguments, one report per line.
fn recording_cockpit(dir: &Path) -> PathBuf {
    common::script(
        dir,
        "cockpit",
        "printf '%s %s\\n' \"$COCKPIT_LAUNCH_TOKEN\" \"$*\" >> \"$(dirname \"$0\")/reports\"",
    )
}

#[test]
#[ignore = "needs an installed Claude Code (COCKPIT_CLAUDE_PROGRAM) and spends one model turn"]
fn a_live_launch_reports_its_hooks_with_its_own_launch_token() {
    let installed = PathBuf::from(
        std::env::var_os("COCKPIT_CLAUDE_PROGRAM")
            .expect("COCKPIT_CLAUDE_PROGRAM names the Claude Code binary to test"),
    );
    let dir = tempfile::tempdir().unwrap();
    let bridge = Bridge {
        hook_program: PathBuf::from(env!("CARGO_BIN_EXE_cockpit-claude-hook")),
        cockpit_program: recording_cockpit(dir.path()),
    };
    let claude = ClaudeCode::discover(&installed, Some(bridge)).unwrap();
    let launch = claude
        .launch(&LaunchRequest {
            working_dir: dir.path(),
            conversation: &Conversation::New,
            first_message: Some("Reply with the single word ok."),
            effort: None,
            remote_control: false,
        })
        .unwrap();

    let status = Command::new(&launch.program)
        .args(["-p", "--model", "haiku"])
        .args(&launch.arguments)
        .envs(launch.environment.iter().cloned())
        .env("COCKPIT_LAUNCH_TOKEN", "live-launch-token")
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .unwrap();

    assert!(status.success(), "{status}");
    let reports = std::fs::read_to_string(dir.path().join("reports")).unwrap();
    assert_eq!(
        reports.lines().collect::<Vec<_>>(),
        [
            "live-launch-token agent report --state waiting",
            "live-launch-token agent report --state working",
            "live-launch-token agent report --state waiting",
        ],
        "SessionStart, UserPromptSubmit, Stop; SessionEnd reports nothing"
    );
}
