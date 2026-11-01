//! `cockpit session` driven as a process against a scripted daemon socket. The crate graph
//! keeps frontends from building the daemon, so the lifecycle itself is proven in
//! `cockpit-daemon/tests/sessions.rs`; these prove each verb's request and its output.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    Command, CoordinatorAuditView, CoordinatorGrantView, ErrorCode, LaunchKindView, LaunchView,
    Outcome, ProtocolError, Reply, SandboxEnforcementView, SandboxEvidenceView, SandboxRuleView,
    SessionFailure, SessionStateView, SessionView, StopTargetView,
};
use scripted::{scripted_daemon, text};

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn runtime_dir(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    fn daemon_socket(&self) -> PathBuf {
        let path = self.runtime_dir().join("cockpit/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    /// `cockpit` with `arguments`, in this home only.
    fn command(&self, arguments: &[&str]) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.runtime_dir());
        command
    }

    /// Runs `cockpit` with `arguments` from `cwd`, off the test's runtime threads.
    async fn cockpit(&self, cwd: &Path, arguments: &[&str]) -> Output {
        let mut command = self.command(arguments);
        command.current_dir(cwd);
        output_off_runtime(command).await
    }
}

async fn output_off_runtime(mut command: std::process::Command) -> Output {
    tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}

fn view(id: &str, state: SessionStateView) -> SessionView {
    SessionView {
        id: id.to_owned(),
        agent: "claude".to_owned(),
        checkout: 1,
        holds_checkout: false,
        working_dir: "/p/app".to_owned(),
        conversation: Some("c-1".to_owned()),
        design_link: None,
        state,
        reason: None,
        revision: 1,
        created_at: 1,
        changed_at: 1,
        launch: Some(LaunchView {
            number: 1,
            kind: LaunchKindView::Start,
            launched_at: 1,
        }),
        changed_start_files: Vec::new(),
        restart_pending: false,
        finished: false,
        coordinator_grants: Vec::new(),
        inbound_message: None,
    }
}

#[tokio::test]
async fn session_new_sends_an_absolute_dir_with_its_options_and_prints_the_session() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Session {
            session: Box::new(view("s0123456789ab", SessionStateView::Running)),
        }),
    );
    let cwd = home.dir.path().canonicalize().unwrap();

    let output = home
        .cockpit(
            &cwd,
            &[
                "session",
                "new",
                "app",
                "--agent",
                "claude",
                "--prompt",
                "fix it",
                "--clone",
                "feature/x",
            ],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s0123456789ab\trunning\tclaude\tstart\tc-1\t/p/app\t-\t-\t-\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionNew {
            dir: cwd.join("app").display().to_string(),
            agent: Some("claude".to_owned()),
            prompt: Some("fix it".to_owned()),
            clone: Some("feature/x".to_owned()),
        }
    );
}

#[tokio::test]
async fn session_list_ended_asks_for_ended_sessions_and_prints_one_line_each() {
    let home = Home::new();
    let mut ended = view("s000000000002", SessionStateView::Ended);
    ended.reason = Some("its agent exited".to_owned());
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Sessions {
            sessions: vec![
                view(
                    "s000000000001",
                    SessionStateView::Stopping {
                        to: StopTargetView::Asleep,
                    },
                ),
                ended,
            ],
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "list", "--ended"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s000000000001\tstopping:asleep\tclaude\tstart\tc-1\t/p/app\t-\t-\t-\n\
         s000000000002\tended\tclaude\tstart\tc-1\t/p/app\tits agent exited\t-\t-\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::SessionList { ended: true });
}

#[tokio::test]
async fn each_one_session_verb_sends_its_command() {
    let session = "s0123456789ab".to_owned();
    let cases: [(&[&str], Command); 6] = [
        (
            &["restart", "s0123456789ab"],
            Command::SessionRestart {
                session: session.clone(),
            },
        ),
        (
            &["sleep", "s0123456789ab"],
            Command::SessionSleep {
                session: session.clone(),
            },
        ),
        (
            &["wake", "s0123456789ab"],
            Command::SessionWake {
                session: session.clone(),
            },
        ),
        (
            &["resume", "s0123456789ab"],
            Command::SessionResume {
                session: session.clone(),
            },
        ),
        (
            &["end", "s0123456789ab"],
            Command::SessionEnd {
                session: session.clone(),
            },
        ),
        (
            &["link", "s0123456789ab", "https://example.com/canvas"],
            Command::SessionLink {
                session: session.clone(),
                url: "https://example.com/canvas".to_owned(),
            },
        ),
    ];
    for (verb, expected) in cases {
        let home = Home::new();
        let daemon = scripted_daemon(
            &home.daemon_socket(),
            Outcome::Ok(Reply::Session {
                session: Box::new(view(&session, SessionStateView::Running)),
            }),
        );
        let arguments: Vec<&str> = ["session"].iter().chain(verb).copied().collect();

        let output = home.cockpit(home.dir.path(), &arguments).await;

        assert!(
            output.status.success(),
            "{verb:?}: {}",
            text(&output.stderr)
        );
        assert_eq!(daemon.await.unwrap(), expected, "{verb:?}");
    }
}

#[tokio::test]
async fn restart_all_exits_1_naming_each_session_it_could_not_restart() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::RestartedAll {
            restarted: vec![view("s000000000001", SessionStateView::Running)],
            deferred: Vec::new(),
            failed: vec![SessionFailure {
                session: "s000000000002".to_owned(),
                error: ProtocolError::new(ErrorCode::Conflict, "it cannot resume"),
            }],
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "restart", "--all"])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stdout),
        "s000000000001\trunning\tclaude\tstart\tc-1\t/p/app\t-\t-\t-\n"
    );
    assert_eq!(
        text(&output.stderr),
        "cockpit: s000000000002 not restarted: conflict: it cannot resume\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::SessionRestartAll);
}

#[tokio::test]
async fn a_refused_session_verb_exits_1_with_the_daemons_reason() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Error(ProtocolError::new(
            ErrorCode::Conflict,
            "session s0123456789ab is running; wake needs asleep",
        )),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "wake", "s0123456789ab"])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "cockpit: conflict: session s0123456789ab is running; wake needs asleep\n"
    );
}

#[tokio::test]
async fn malformed_session_arguments_exit_2_before_any_connection() {
    let home = Home::new();
    let cases: [&[&str]; 6] = [
        &["session"],
        &["session", "new"],
        &["session", "new", "app", "--agent"],
        &["session", "new", "app", "--clone", "a", "--clone", "b"],
        &["session", "new", "app", "--model", "x"],
        &["session", "link", "s0123456789ab"],
    ];
    for arguments in cases {
        let output = home.cockpit(home.dir.path(), arguments).await;

        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(
            text(&output.stderr).contains("usage:"),
            "{arguments:?}: {}",
            text(&output.stderr)
        );
    }
}

// #157 (decided in #201): which sessions are finished is the daemon's rule, proven in
// `cockpit-daemon/tests/session_robustness.rs`; these prove the column and the verb.

#[tokio::test]
async fn session_list_marks_a_finished_session_in_its_work_column() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Sessions {
            sessions: vec![
                SessionView {
                    finished: true,
                    coordinator_grants: Vec::new(),
                    inbound_message: None,
                    ..view("s000000000001", SessionStateView::Running)
                },
                view("s000000000002", SessionStateView::Running),
            ],
        }),
    );

    let output = home.cockpit(home.dir.path(), &["session", "list"]).await;

    assert_eq!(
        text(&output.stdout),
        "s000000000001\trunning\tclaude\tstart\tc-1\t/p/app\t-\t-\tfinished\n\
         s000000000002\trunning\tclaude\tstart\tc-1\t/p/app\t-\t-\t-\n"
    );
}

#[tokio::test]
async fn end_finished_sends_one_request_and_prints_each_ended_session() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::EndedFinished {
            ended: vec![view("s000000000001", SessionStateView::Ended)],
            failed: Vec::new(),
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "end", "--finished"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s000000000001\tended\tclaude\tstart\tc-1\t/p/app\t-\t-\t-\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionEndFinished { only: None }
    );
}

#[tokio::test]
async fn end_finished_exits_1_naming_each_session_it_could_not_end() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::EndedFinished {
            ended: Vec::new(),
            failed: vec![SessionFailure {
                session: "s000000000002".to_owned(),
                error: ProtocolError::new(ErrorCode::Conflict, "its agent did not stop"),
            }],
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "end", "--finished"])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "cockpit: s000000000002 not ended: conflict: its agent did not stop\n"
    );
}

// #275: `cockpit session list | head -1` must not panic once `head` has exited.
#[tokio::test]
async fn session_list_exits_141_without_a_panic_when_its_stdout_is_closed() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Sessions {
            sessions: vec![view("s000000000001", SessionStateView::Running)],
        }),
    );
    let (closed_reader, writer) = std::io::pipe().unwrap();
    drop(closed_reader);
    let mut command = home.command(&["session", "list"]);
    command.stdout(writer);

    let output = output_off_runtime(command).await;

    assert_eq!(text(&output.stderr), "");
    assert_eq!(output.status.code(), Some(141));
}

// #61: how a session ran and what a new one would get are the daemon's to decide, proven in
// `cockpit-daemon/tests/sandbox.rs`; these prove the verbs' requests and their lines.

fn evidence() -> SandboxEvidenceView {
    SandboxEvidenceView {
        enforcement: SandboxEnforcementView::Sandboxed {
            backend: "greywall".to_owned(),
            engine: "greywall 0.3.7 (7c06d51), greyproxy 0.4.5 (cdd5cde)".to_owned(),
            client_identity: "cockpit-s0123456789ab-0a1b2c3d4e5f".to_owned(),
        },
        platform: "linux".to_owned(),
        rules: vec![SandboxRuleView {
            rule: "read-write /p/app".to_owned(),
            enforced_by: "greywall bubblewrap+Landlock (--allow-path)".to_owned(),
        }],
        observed: vec!["landlock: yes".to_owned()],
    }
}

#[tokio::test]
async fn session_sandbox_prints_the_evidence_of_the_latest_launch() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::SessionSandbox {
            evidence: Some(Box::new(evidence())),
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "sandbox", "s0123456789ab"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "sandbox: enforced by greywall on linux\n\
         engine: greywall 0.3.7 (7c06d51), greyproxy 0.4.5 (cdd5cde)\n\
         proxy client: cockpit-s0123456789ab-0a1b2c3d4e5f\n\
         rule: read-write /p/app <- greywall bubblewrap+Landlock (--allow-path)\n\
         observed: landlock: yes\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionSandbox {
            session: "s0123456789ab".to_owned()
        }
    );
}

#[tokio::test]
async fn session_sandbox_says_when_no_launch_is_recorded() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::SessionSandbox { evidence: None }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "sandbox", "s0123456789ab"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "sandbox: no launch recorded yet\n");
}

#[tokio::test]
async fn sandbox_show_sends_an_absolute_dir_and_prints_the_profile_and_command() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::SandboxProfile {
            evidence: Box::new(evidence()),
            command: vec![
                "/usr/bin/greywall".to_owned(),
                "--".to_owned(),
                "<agent>".to_owned(),
            ],
        }),
    );
    let cwd = home.dir.path().canonicalize().unwrap();

    let output = home
        .cockpit(&cwd, &["sandbox", "show", "app", "--agent", "codex"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stdout)
            .ends_with("observed: landlock: yes\ncommand: /usr/bin/greywall -- <agent>\n"),
        "{}",
        text(&output.stdout)
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::SandboxShow {
            dir: cwd.join("app").display().to_string(),
            agent: Some("codex".to_owned()),
        }
    );
}

#[tokio::test]
async fn a_refused_sandbox_show_exits_1_with_the_rule_and_reason() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Error(ProtocolError::new(
            ErrorCode::Refused,
            "the sandbox refuses `sandbox`: no sandbox engine: no `greywall` on PATH",
        )),
    );

    let output = home
        .cockpit(home.dir.path(), &["sandbox", "show", "/p/app"])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "cockpit: refused: the sandbox refuses `sandbox`: no sandbox engine: no `greywall` on PATH\n"
    );
}

#[tokio::test]
async fn malformed_sandbox_arguments_exit_2_before_any_connection() {
    let home = Home::new();
    let cases: [&[&str]; 4] = [
        &["sandbox"],
        &["sandbox", "show"],
        &["sandbox", "show", "/p/app", "--agent"],
        &["session", "sandbox"],
    ];
    for arguments in cases {
        let output = home.cockpit(home.dir.path(), arguments).await;

        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(
            text(&output.stderr).contains("usage:"),
            "{arguments:?}: {}",
            text(&output.stderr)
        );
    }
}

// #410: the owner's coordinator verbs; which permission takes which terms is the daemon's
// rule, proven in `cockpit-daemon/tests/coordinator_kit.rs`.

#[tokio::test]
async fn session_grant_sends_the_permission_and_its_start_terms() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::CoordinatorGrants {
            session: "s0123456789ab".to_owned(),
            grants: Vec::new(),
            audit: Vec::new(),
        }),
    );

    let output = home
        .cockpit(
            home.dir.path(),
            &[
                "session",
                "grant",
                "s0123456789ab",
                "start-sessions",
                "--per-hour",
                "3",
                "--without-confirmation",
            ],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionGrant {
            session: "s0123456789ab".to_owned(),
            permission: "start-sessions".to_owned(),
            without_confirmation: true,
            per_hour: Some(3),
        }
    );
}

#[tokio::test]
async fn session_grants_prints_each_grant_then_the_audit_newest_first() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::CoordinatorGrants {
            session: "s0123456789ab".to_owned(),
            grants: vec![
                CoordinatorGrantView {
                    permission: "observe-sessions".to_owned(),
                    grant: 4,
                    per_hour: None,
                    needs_confirmation: None,
                },
                CoordinatorGrantView {
                    permission: "start-sessions".to_owned(),
                    grant: 5,
                    per_hour: Some(10),
                    needs_confirmation: Some(true),
                },
            ],
            audit: vec![
                CoordinatorAuditView {
                    id: 9,
                    actor: "s0123456789ab".to_owned(),
                    permission: "start-sessions".to_owned(),
                    action: "start".to_owned(),
                    target: None,
                    detail: Some("approval request 2".to_owned()),
                    at: 1_700_000_100,
                },
                CoordinatorAuditView {
                    id: 8,
                    actor: "s0123456789ab".to_owned(),
                    permission: "message-sessions".to_owned(),
                    action: "message".to_owned(),
                    target: Some("s000000000002".to_owned()),
                    detail: Some("rebase on main".to_owned()),
                    at: 1_700_000_000,
                },
            ],
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "grants", "s0123456789ab"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "grant\tobserve-sessions\t4\t-\n\
         grant\tstart-sessions\t5\tper-hour=10 confirmed\n\
         audit\t9\t1700000100\ts0123456789ab\tstart\t-\t\"approval request 2\"\n\
         audit\t8\t1700000000\ts0123456789ab\tmessage\ts000000000002\t\"rebase on main\"\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionGrants {
            session: "s0123456789ab".to_owned(),
        }
    );
}

#[tokio::test]
async fn session_revoke_sends_the_permission_to_take_back() {
    let home = Home::new();
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::CoordinatorGrants {
            session: "s0123456789ab".to_owned(),
            grants: Vec::new(),
            audit: Vec::new(),
        }),
    );

    let output = home
        .cockpit(
            home.dir.path(),
            &["session", "revoke", "s0123456789ab", "message-sessions"],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionRevoke {
            session: "s0123456789ab".to_owned(),
            permission: "message-sessions".to_owned(),
        }
    );
}

#[tokio::test]
async fn malformed_grant_arguments_exit_2_before_any_connection() {
    let home = Home::new();
    let cases: [&[&str]; 6] = [
        &["session", "grant", "s0123456789ab"],
        &[
            "session",
            "grant",
            "s0123456789ab",
            "start-sessions",
            "--per-hour",
        ],
        &[
            "session",
            "grant",
            "s0123456789ab",
            "start-sessions",
            "--per-hour",
            "many",
        ],
        &[
            "session",
            "grant",
            "s0123456789ab",
            "start-sessions",
            "--without-confirmation",
            "--without-confirmation",
        ],
        &["session", "revoke", "s0123456789ab"],
        &["session", "grants"],
    ];
    for arguments in cases {
        let output = home.cockpit(home.dir.path(), arguments).await;

        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(
            text(&output.stderr).contains("usage:"),
            "{arguments:?}: {}",
            text(&output.stderr)
        );
    }
}
