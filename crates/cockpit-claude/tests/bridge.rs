//! The in-sandbox hook bridge, run as Claude Code runs it: the payload on standard input,
//! the `cockpit` program as its argument. A fake `cockpit` records what it was asked.

mod common;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// What the fake `cockpit brief` prints: the brief, then the task packet of the step the
/// session does (#781).
const BRIEF: &str = "# Cockpit launch brief\n\n\
# Task packet\n\n\
Your step as `torlek workflow packet 7` prints it, with only the last handoff.\n\n\
workflow 7 (code-change v1, running)\n\
step 2 check (active, s0123456789ab) [--step 2 --attempt 1]: tests pass, review next";

struct FakeCockpit {
    dir: tempfile::TempDir,
}

impl FakeCockpit {
    /// A `cockpit` that records its arguments and launch token, then exits with `status`;
    /// `cockpit brief` prints [`BRIEF`] and exits 0.
    fn exiting_with(status: i32) -> FakeCockpit {
        FakeCockpit::answering_brief_with(status, 0)
    }

    /// A `cockpit` whose reports exit with `status` and whose `brief` exits with
    /// `brief_status`.
    fn answering_brief_with(status: i32, brief_status: i32) -> FakeCockpit {
        let dir = tempfile::tempdir().unwrap();
        common::script(
            dir.path(),
            "cockpit",
            &format!(
                "if [ \"$1\" = brief ]; then printf '%s' \"$TORLEK_LAUNCH_TOKEN\" \
                   > \"$(dirname \"$0\")/brief-token\"\n\
                   printf '{BRIEF}\\n'; exit {brief_status}; fi\n\
                 printf '%s\\n' \"$@\" > \"$(dirname \"$0\")/arguments\"\n\
                 printf '%s' \"$TORLEK_LAUNCH_TOKEN\" > \"$(dirname \"$0\")/token\"\n\
                 echo 'reported for session s000000000001'\nexit {status}"
            ),
        );
        FakeCockpit { dir }
    }

    fn program(&self) -> PathBuf {
        self.dir.path().join("cockpit")
    }

    /// The arguments it was run with; `None` when it never ran.
    fn arguments(&self) -> Option<Vec<String>> {
        let text = std::fs::read_to_string(self.dir.path().join("arguments")).ok()?;
        Some(text.lines().map(str::to_owned).collect())
    }

    fn token(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("token")).unwrap()
    }
}

fn run_hook(cockpit: &Path, payload: &str, token: Option<&str>) -> Output {
    run_hook_in(cockpit, payload, token, &[])
}

/// Runs the bridge with `environment` set over an environment that holds no sign-in of its
/// own, no `HOME` and no `CLAUDE_CODE_OAUTH_TOKEN`, so no test reaches the owner's sign-in;
/// a test that gives a home puts its own `curl` first on `PATH`.
fn run_hook_in(
    cockpit: &Path,
    payload: &str,
    token: Option<&str>,
    environment: &[(&str, &str)],
) -> Output {
    let mut hook = Command::new(env!("CARGO_BIN_EXE_torlek-claude-hook"));
    hook.arg(cockpit)
        .env_remove("TORLEK_LAUNCH_TOKEN")
        .env_remove("HOME")
        .env_remove("CLAUDE_CODE_OAUTH_TOKEN")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(token) = token {
        hook.env("TORLEK_LAUNCH_TOKEN", token);
    }
    for (name, value) in environment {
        hook.env(name, value);
    }
    let mut child = hook.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn a_stop_reports_a_completed_turn_through_the_cockpit_program() {
    let cockpit = FakeCockpit::exiting_with(0);
    let output = run_hook(
        &cockpit.program(),
        &common::fixture("stop"),
        Some("token-1"),
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        cockpit.arguments().unwrap(),
        [
            "agent",
            "report",
            "--state",
            "waiting",
            "--outcome",
            "completed",
            "--conversation",
            "49d695c9-4342-47df-a3fc-9cec34484f36"
        ]
    );
    assert_eq!(cockpit.token(), "token-1");
}

#[test]
fn the_reports_answer_never_reaches_claudes_context() {
    let cockpit = FakeCockpit::exiting_with(0);
    let output = run_hook(&cockpit.program(), &common::fixture("stop"), None);

    assert!(output.status.success(), "{output:?}");
    assert!(
        output.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&output.stdout)
    );
}

/// #419: every context Claude Code starts, a compaction's included, gets the launch brief
/// without the agent asking for it.
#[test]
fn each_session_start_adds_the_launch_brief_to_claudes_context() {
    for fixture in [
        "session-start",
        "session-start-clear",
        "session-start-compact",
    ] {
        let cockpit = FakeCockpit::exiting_with(0);
        let output = run_hook(
            &cockpit.program(),
            &common::fixture(fixture),
            Some("token-1"),
        );

        assert!(output.status.success(), "{fixture}: {output:?}");
        let answer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            answer,
            serde_json::json!({
                "hookSpecificOutput": {
                    "hookEventName": "SessionStart",
                    "additionalContext": format!("{BRIEF}\n"),
                }
            }),
            "{fixture}"
        );
        assert_eq!(
            std::fs::read_to_string(cockpit.dir.path().join("brief-token")).unwrap(),
            "token-1",
            "{fixture}"
        );
    }
}

/// #781: the pinned `source: "compact"` payload gets the brief and the task packet back
/// whole, so a compaction's summary is never the only record of the step.
#[test]
fn a_compactions_session_start_adds_the_brief_and_task_packet() {
    let cockpit = FakeCockpit::exiting_with(0);
    let payload = common::fixture("session-start-compact");
    assert!(payload.contains(r#""source":"compact""#), "{payload}");

    let output = run_hook(&cockpit.program(), &payload, Some("token-1"));

    assert!(output.status.success(), "{output:?}");
    let answer: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let context = answer["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(context, format!("{BRIEF}\n"));
    assert!(context.contains("# Task packet"), "{context}");
}

/// The report and the brief run side by side; when both fail, neither failure is hidden.
#[test]
fn a_start_whose_report_and_brief_both_fail_names_both() {
    let cockpit = FakeCockpit::answering_brief_with(3, 3);
    let output = run_hook(&cockpit.program(), &common::fixture("session-start"), None);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("`torlek agent report"), "{stderr}");
    assert!(stderr.contains("`torlek brief` failed"), "{stderr}");
}

#[test]
fn a_brief_that_cannot_be_read_still_lets_the_start_report_and_exits_1() {
    let cockpit = FakeCockpit::answering_brief_with(0, 3);
    let output = run_hook(&cockpit.program(), &common::fixture("session-start"), None);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("`torlek brief` failed"),
        "{output:?}"
    );
    assert_eq!(
        cockpit.arguments().unwrap()[..3],
        ["agent", "report", "--state"]
    );
}

#[test]
fn a_clear_announces_the_conversation_ending_through_the_cockpit_program() {
    let cockpit = FakeCockpit::exiting_with(0);
    let output = run_hook(
        &cockpit.program(),
        &common::fixture("session-end-clear"),
        Some("token-1"),
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        cockpit.arguments().unwrap(),
        [
            "agent",
            "report",
            "--conversation",
            "003eb9a6-45b4-40b6-9555-20f4c6c02655",
            "--conversation-ends"
        ]
    );
}

#[test]
fn an_event_without_a_report_runs_nothing() {
    let cockpit = FakeCockpit::exiting_with(0);
    let output = run_hook(&cockpit.program(), &common::fixture("session-end"), None);

    assert!(output.status.success(), "{output:?}");
    assert_eq!(cockpit.arguments(), None);
}

/// Exit 2 would block Claude Code's stop or prompt; a failed report must only be shown.
#[test]
fn a_refused_report_exits_1_and_never_2() {
    let cockpit = FakeCockpit::exiting_with(2);
    let output = run_hook(&cockpit.program(), &common::fixture("stop"), None);

    assert_eq!(output.status.code(), Some(1), "{output:?}");
}

#[test]
fn a_payload_that_is_not_a_hook_event_exits_1_saying_why() {
    let cockpit = FakeCockpit::exiting_with(0);
    let output = run_hook(&cockpit.program(), "not json", None);

    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).starts_with("torlek-claude-hook: "),
        "{output:?}"
    );
    assert_eq!(cockpit.arguments(), None);
}

#[test]
fn a_missing_cockpit_program_exits_1() {
    let output = run_hook(
        Path::new("/nonexistent/cockpit"),
        &common::fixture("stop"),
        None,
    );

    assert_eq!(output.status.code(), Some(1), "{output:?}");
}

/// #782: a compaction's two hooks reach the agent channel as compaction reports, with the
/// sizes the transcript holds, and print nothing for Claude Code's context.
#[test]
fn a_compaction_is_reported_through_the_cockpit_program_with_its_sizes() {
    let directory = tempfile::tempdir().unwrap();
    let transcript = directory.path().join("transcript.jsonl");
    std::fs::copy(common::transcript("transcript-compaction"), &transcript).unwrap();
    for (fixture, expected) in [
        (
            "pre-compact",
            vec![
                "--compaction",
                "started",
                "--compaction-tokens-before",
                "268012",
            ],
        ),
        (
            "post-compact",
            vec![
                "--compaction",
                "ended",
                "--compaction-tokens-after",
                "23678",
            ],
        ),
    ] {
        let mut payload: serde_json::Value =
            serde_json::from_str(&common::fixture(fixture)).unwrap();
        payload["transcript_path"] = transcript.to_str().unwrap().into();
        let cockpit = FakeCockpit::exiting_with(0);

        let output = run_hook(&cockpit.program(), &payload.to_string(), Some("token-1"));

        assert!(output.status.success(), "{fixture}: {output:?}");
        assert!(output.stdout.is_empty(), "{fixture}");
        let arguments = cockpit.arguments().unwrap();
        assert_eq!(arguments[..2], ["agent", "report"], "{fixture}");
        for pair in expected.windows(2).step_by(2) {
            assert!(
                arguments.windows(2).any(|found| found == pair),
                "{fixture}: {pair:?} in {arguments:?}"
            );
        }
        assert_eq!(cockpit.token(), "token-1");
    }
}

/// #479: a turn's end reaches the agent channel with the harness's context, read from the
/// transcript's last main-chain model call; Claude Code names no window.
#[test]
fn a_turns_end_is_reported_with_the_context_the_transcript_holds() {
    let directory = tempfile::tempdir().unwrap();
    let transcript = directory.path().join("transcript.jsonl");
    std::fs::copy(common::transcript("transcript-turn"), &transcript).unwrap();
    let mut payload: serde_json::Value = serde_json::from_str(&common::fixture("stop")).unwrap();
    payload["transcript_path"] = transcript.to_str().unwrap().into();
    let cockpit = FakeCockpit::exiting_with(0);

    let output = run_hook(&cockpit.program(), &payload.to_string(), Some("token-1"));

    assert!(output.status.success(), "{output:?}");
    let arguments = cockpit.arguments().unwrap();
    assert!(
        arguments
            .windows(2)
            .any(|found| found == ["--context-tokens", "45123"]),
        "{arguments:?}"
    );
    assert!(
        !arguments
            .iter()
            .any(|argument| argument == "--context-window"),
        "{arguments:?}"
    );
}

/// A session home with Claude Code's sign-in and, on its `PATH` folder, a `curl` that
/// records its standard input and answers the usage endpoint with a saved answer.
struct SessionHome {
    dir: tempfile::TempDir,
}

impl SessionHome {
    fn signed_in() -> SessionHome {
        let home = SessionHome::empty();
        let folder = home.path().join(".claude");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join(".credentials.json"),
            r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-session-token","expiresAt":4102444800000,"subscriptionType":"max"}}"#,
        )
        .unwrap();
        home
    }

    fn empty() -> SessionHome {
        SessionHome {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    /// A `PATH` whose first `curl` prints the usage answer `fixture` with HTTP 200.
    fn curl_answering(&self, fixture: &str) -> String {
        let bin = self.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let body = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/oauth-usage")
            .join(format!("{fixture}.json"));
        common::script(
            &bin,
            "curl",
            &format!(
                "cat > '{}/curl-input'\ncat '{}'; printf '\\n200'",
                self.path().display(),
                body.display()
            ),
        );
        format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    fn curl_input(&self) -> String {
        std::fs::read_to_string(self.path().join("curl-input")).unwrap()
    }
}

fn outcome_of(arguments: &[String]) -> Vec<&str> {
    let at = arguments
        .iter()
        .position(|argument| argument == "--outcome")
        .unwrap_or_else(|| panic!("no outcome in {arguments:?}"));
    arguments[at..]
        .iter()
        .take_while(|argument| *argument != "--conversation")
        .map(String::as_str)
        .collect()
}

#[test]
fn a_rate_limit_stop_with_a_spent_window_is_reported_as_a_quota_stop() {
    let home = SessionHome::signed_in();
    let path = home.curl_answering("limits-spent");
    let cockpit = FakeCockpit::exiting_with(0);

    let output = run_hook_in(
        &cockpit.program(),
        &common::fixture("stop-failure-rate-limit-plan-window"),
        Some("token-1"),
        &[("HOME", home.path().to_str().unwrap()), ("PATH", &path)],
    );

    assert!(output.status.success(), "{output:?}");
    let arguments = cockpit.arguments().unwrap();
    assert_eq!(outcome_of(&arguments), ["--outcome", "quota-stopped"]);
    assert!(
        home.curl_input()
            .contains("Authorization: Bearer sk-ant-oat01-session-token"),
        "{}",
        home.curl_input()
    );
    assert!(output.stdout.is_empty(), "{output:?}");
}

#[test]
fn a_rate_limit_stop_with_headroom_is_reported_as_a_failure() {
    let home = SessionHome::signed_in();
    let path = home.curl_answering("limits");
    let cockpit = FakeCockpit::exiting_with(0);

    let output = run_hook_in(
        &cockpit.program(),
        &common::fixture("stop-failure-rate-limit-transient"),
        Some("token-1"),
        &[("HOME", home.path().to_str().unwrap()), ("PATH", &path)],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        outcome_of(&cockpit.arguments().unwrap()),
        ["--outcome", "failed", "--outcome-reason", "rate_limit"]
    );
}

#[test]
fn a_bound_credential_asks_for_the_windows_with_the_token_the_session_holds() {
    let home = SessionHome::empty();
    let path = home.curl_answering("limits-spent");
    let cockpit = FakeCockpit::exiting_with(0);

    let output = run_hook_in(
        &cockpit.program(),
        &common::fixture("stop-failure-rate-limit-plan-window"),
        Some("token-1"),
        &[
            ("HOME", home.path().to_str().unwrap()),
            ("PATH", &path),
            ("CLAUDE_CODE_OAUTH_TOKEN", "torlek-placeholder"),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        outcome_of(&cockpit.arguments().unwrap()),
        ["--outcome", "quota-stopped"]
    );
    assert_eq!(
        home.curl_input(),
        "Authorization: Bearer torlek-placeholder\n"
    );
}

#[test]
fn an_empty_token_variable_leaves_the_homes_sign_in_to_ask() {
    let home = SessionHome::signed_in();
    let path = home.curl_answering("limits-spent");
    let cockpit = FakeCockpit::exiting_with(0);

    let output = run_hook_in(
        &cockpit.program(),
        &common::fixture("stop-failure-rate-limit-plan-window"),
        Some("token-1"),
        &[
            ("HOME", home.path().to_str().unwrap()),
            ("PATH", &path),
            ("CLAUDE_CODE_OAUTH_TOKEN", ""),
        ],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        outcome_of(&cockpit.arguments().unwrap()),
        ["--outcome", "quota-stopped"]
    );
    assert_eq!(
        home.curl_input(),
        "Authorization: Bearer sk-ant-oat01-session-token\n"
    );
}

#[test]
fn a_rate_limit_stop_whose_windows_cannot_be_read_stays_a_failure_and_says_why() {
    let home = SessionHome::empty();
    let cockpit = FakeCockpit::exiting_with(0);

    let output = run_hook_in(
        &cockpit.program(),
        &common::fixture("stop-failure-rate-limit-plan-window"),
        Some("token-1"),
        &[("HOME", home.path().to_str().unwrap())],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        outcome_of(&cockpit.arguments().unwrap()),
        ["--outcome", "failed", "--outcome-reason", "rate_limit"]
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("the plan windows could not be read, so the rate limit stays a failure")
            && stderr.contains("not signed in"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty(), "{output:?}");
}

#[test]
fn a_completed_turn_never_asks_for_the_plan_windows() {
    let home = SessionHome::signed_in();
    let path = home.curl_answering("limits-spent");
    let cockpit = FakeCockpit::exiting_with(0);

    let output = run_hook_in(
        &cockpit.program(),
        &common::fixture("stop"),
        Some("token-1"),
        &[("HOME", home.path().to_str().unwrap()), ("PATH", &path)],
    );

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        outcome_of(&cockpit.arguments().unwrap()),
        ["--outcome", "completed"]
    );
    assert!(!home.path().join("curl-input").exists());
}

/// #718: the payload and transcript tail Claude Code 2.1.319 wrote for a turn whose API
/// connection was refused, captured with its transcript copied as the hook ran; the payload
/// names that copy.
fn connection_failed_stop(transcript_tail: &str) -> (tempfile::TempDir, String) {
    let directory = tempfile::tempdir().unwrap();
    let transcript = directory.path().join("transcript.jsonl");
    std::fs::write(&transcript, transcript_tail).unwrap();
    let mut payload: serde_json::Value = serde_json::from_str(&common::captured(
        "2.1.319",
        "stop-failure-connection-failed.json",
    ))
    .unwrap();
    payload["transcript_path"] = transcript.to_str().unwrap().into();
    (directory, payload.to_string())
}

fn captured_connection_failed_tail() -> String {
    common::captured("2.1.319", "transcript-connection-failed.jsonl")
}

fn reported_outcome(payload: &str) -> Vec<String> {
    let cockpit = FakeCockpit::exiting_with(0);
    let output = run_hook(&cockpit.program(), payload, Some("token-1"));
    assert!(output.status.success(), "{output:?}");
    outcome_of(&cockpit.arguments().unwrap())
        .into_iter()
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_turn_whose_api_connection_failed_is_reported_as_network_lost() {
    let (_directory, payload) = connection_failed_stop(&captured_connection_failed_tail());

    assert_eq!(
        reported_outcome(&payload),
        [
            "--outcome",
            "network-lost",
            "--outcome-reason",
            "connection_failed"
        ]
    );
}

#[test]
fn a_server_error_whose_turn_ended_on_another_api_error_stays_a_failure() {
    let tail = captured_connection_failed_tail().replace(r#""apiError":"connection_failed","#, "");
    let (_directory, payload) = connection_failed_stop(&tail);

    assert_eq!(
        reported_outcome(&payload),
        ["--outcome", "failed", "--outcome-reason", "server_error"]
    );
}

#[test]
fn a_connection_failure_of_an_earlier_turn_leaves_this_failure_as_it_is() {
    let mut tail = captured_connection_failed_tail();
    tail.push_str(
        r#"{"type":"user","isSidechain":false,"message":{"role":"user","content":"try again"}}"#,
    );
    tail.push('\n');
    let (_directory, payload) = connection_failed_stop(&tail);

    assert_eq!(
        reported_outcome(&payload),
        ["--outcome", "failed", "--outcome-reason", "server_error"]
    );
}

#[test]
fn a_server_error_whose_transcript_cannot_be_read_stays_a_failure() {
    let payload = common::captured("2.1.319", "stop-failure-connection-failed.json");

    assert_eq!(
        reported_outcome(&payload),
        ["--outcome", "failed", "--outcome-reason", "server_error"]
    );
}

/// The transcript is written asynchronously: an API error Claude Code appends just after the
/// hook starts still counts.
#[test]
fn a_connection_failure_written_just_after_the_hook_starts_is_still_network_lost() {
    let tail = captured_connection_failed_tail();
    let (error_at, _) = tail
        .match_indices('\n')
        .find(|(at, _)| tail[..*at].ends_with('}') && tail[at + 1..].contains("connection_failed"))
        .unwrap();
    let (directory, payload) = connection_failed_stop(&tail[..=error_at]);
    let transcript = directory.path().join("transcript.jsonl");
    let rest = tail[error_at + 1..].to_owned();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(transcript)
            .unwrap();
        file.write_all(rest.as_bytes()).unwrap();
    });

    let outcome = reported_outcome(&payload);

    writer.join().unwrap();
    assert_eq!(
        outcome,
        [
            "--outcome",
            "network-lost",
            "--outcome-reason",
            "connection_failed"
        ]
    );
}

#[test]
fn a_meta_record_after_the_connection_failure_leaves_it_network_lost() {
    let mut tail = captured_connection_failed_tail();
    tail.push_str(
        r#"{"type":"user","isSidechain":false,"isMeta":true,"message":{"role":"user","content":"<local-command-caveat>"}}"#,
    );
    tail.push('\n');
    let (_directory, payload) = connection_failed_stop(&tail);

    assert_eq!(
        reported_outcome(&payload),
        [
            "--outcome",
            "network-lost",
            "--outcome-reason",
            "connection_failed"
        ]
    );
}
