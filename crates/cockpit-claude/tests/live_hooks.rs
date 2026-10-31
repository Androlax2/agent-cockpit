//! The hook bridge against a live Claude Code (#184): a real launch's hooks report with the
//! launch token they inherit. Ignored by default because CI has no Claude Code and it
//! spends one short model turn; run it with the binary of `CONFORMANCE_VERSION`:
//!
//! ```text
//! COCKPIT_CLAUDE_PROGRAM=~/.local/share/claude/versions/2.1.316 \
//!   cargo nextest run -p cockpit-claude --test live_hooks --run-ignored only
//! ```

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cockpit_claude::{Bridge, ClaudeCode};
use cockpit_provider_api::{Conversation, LaunchRequest, Provider};

/// How long the live turn may take before the test gives up on it.
const TURN_DEADLINE: Duration = Duration::from_secs(120);

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

    // Without the owner's settings their own hooks stay off; without the parent's session
    // variables nothing this turn does can report into the session running the test.
    let mut child = Command::new(&launch.program)
        .args(["-p", "--model", "haiku", "--setting-sources", "project"])
        .args(&launch.arguments)
        .envs(launch.environment.iter().cloned())
        .env_remove("COCKPIT_SESSION")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env("COCKPIT_LAUNCH_TOKEN", "live-launch-token")
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > TURN_DEADLINE {
            child.kill().unwrap();
            panic!("the live turn took over {TURN_DEADLINE:?}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let output = child.wait_with_output().unwrap();

    assert!(output.status.success(), "{output:?}");
    let reports = std::fs::read_to_string(dir.path().join("reports"))
        .unwrap_or_else(|error| panic!("no hook reported ({error}): {output:?}"));
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
