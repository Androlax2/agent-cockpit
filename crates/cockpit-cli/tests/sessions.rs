//! `cockpit session` driven as a process against a scripted daemon socket. The crate graph
//! keeps frontends from building the daemon, so the lifecycle itself is proven in
//! `cockpit-daemon/tests/sessions.rs`; these prove each verb's request and its output.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Output;

mod scripted;

use cockpit_protocol::{
    AgentExitView, Command, CoordinatorAuditView, CoordinatorGrantView, ErrorCode, ExitStatusView,
    HeldMessageView, LaunchKindView, LaunchView, Outcome, ProtocolError, QuotaStopStatusView,
    QuotaStopView, Reply, SandboxEnforcementView, SandboxEvidenceView, SandboxRuleView,
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
        let path = self.runtime_dir().join("torlek-dev/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    /// `cockpit` with `arguments`, in this home only.
    fn command(&self, arguments: &[&str]) -> std::process::Command {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_torlek"));
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
            account: None,
            switched_from: None,
            passed_over: Vec::new(),
        }),
        exit: None,
        changed_start_files: Vec::new(),
        restart_pending: false,
        finished: false,
        coordinator_grants: Vec::new(),
        inbound_message: None,
        compaction_threshold: None,
        sign_in_needed: None,
        quota_stop: None,
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
            model: None,
            effort: None,
            compaction_threshold: None,
        }
    );
}

#[tokio::test]
async fn session_new_sends_the_chosen_model_and_effort_as_typed() {
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
                "--model",
                "claude-opus-5-5",
                "--effort",
                "xhigh",
            ],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionNew {
            dir: cwd.join("app").display().to_string(),
            agent: None,
            prompt: None,
            clone: None,
            model: Some("claude-opus-5-5".to_owned()),
            effort: Some("xhigh".to_owned()),
            compaction_threshold: None,
        }
    );
}

#[tokio::test]
async fn session_new_sends_the_chosen_compaction_threshold_as_typed() {
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
            &["session", "new", "app", "--compaction-threshold", "200k"],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionNew {
            dir: cwd.join("app").display().to_string(),
            agent: None,
            prompt: None,
            clone: None,
            model: None,
            effort: None,
            compaction_threshold: Some("200k".to_owned()),
        }
    );
}

/// #59: `session show` names the account the latest launch runs on.
#[tokio::test]
async fn session_show_prints_the_account_its_launch_runs_on() {
    let home = Home::new();
    let mut shown = view("s0123456789ab", SessionStateView::Running);
    shown.launch.as_mut().unwrap().account = Some("work".to_owned());
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Session {
            session: Box::new(shown),
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "show", "s0123456789ab"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s0123456789ab\trunning\tclaude\tstart\tc-1\t/p/app\t-\t-\t-\naccount: work\n"
    );
}

#[tokio::test]
async fn session_show_prints_the_account_a_quota_stop_moved_its_launch_off() {
    let home = Home::new();
    let mut shown = view("s0123456789ab", SessionStateView::Running);
    let launch = shown.launch.as_mut().unwrap();
    launch.account = Some("spare".to_owned());
    launch.switched_from = Some("work".to_owned());
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Session {
            session: Box::new(shown),
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "show", "s0123456789ab"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(
        text(&output.stdout).ends_with("account: spare (switched from work at a quota stop)\n"),
        "{}",
        text(&output.stdout)
    );
}

/// One `session show` of a running session whose last quota stop is `stop`, in UTC.
async fn shown_quota_stop(stop: QuotaStopView) -> String {
    let home = Home::new();
    let mut shown = view("s0123456789ab", SessionStateView::Running);
    shown.quota_stop = Some(stop);
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Session {
            session: Box::new(shown),
        }),
    );
    let mut command = home.command(&["session", "show", "s0123456789ab"]);
    command.current_dir(home.dir.path()).env("TZ", "UTC");
    let output = output_off_runtime(command).await;
    assert!(output.status.success(), "{}", text(&output.stderr));
    text(&output.stdout)
}

/// 2026-11-09 14:00:00 UTC.
const STOPPED_AT: i64 = 1_794_232_800;

#[tokio::test]
async fn session_show_prints_each_quota_stop_with_where_its_resume_stands() {
    let hour = 3600;
    let cases = [
        (
            None,
            None,
            None,
            QuotaStopStatusView::Waiting,
            "quota stop: 2026-11-09 14:00:00 +00:00 on the owner's sign-in; resumes after the plan's reset\n",
        ),
        (
            None,
            Some(STOPPED_AT + 5 * hour),
            None,
            QuotaStopStatusView::Waiting,
            "quota stop: 2026-11-09 14:00:00 +00:00 on the owner's sign-in; resumes after the plan's reset at 2026-11-09 19:00:00 +00:00\n",
        ),
        (
            Some("work"),
            None,
            Some(STOPPED_AT),
            QuotaStopStatusView::Waiting,
            "quota stop: 2026-11-09 14:00:00 +00:00 on account work; resumes once its agent waits on its new account\n",
        ),
        (
            Some("work"),
            None,
            Some(STOPPED_AT),
            QuotaStopStatusView::Resumed {
                at: STOPPED_AT + 60,
            },
            "quota stop: 2026-11-09 14:00:00 +00:00 on account work; resumed at 2026-11-09 14:01:00 +00:00 on its new account\n",
        ),
        (
            None,
            Some(STOPPED_AT + 5 * hour),
            None,
            QuotaStopStatusView::Resumed {
                at: STOPPED_AT + 5 * hour,
            },
            "quota stop: 2026-11-09 14:00:00 +00:00 on the owner's sign-in; resumed at 2026-11-09 19:00:00 +00:00 after the plan's reset\n",
        ),
        (
            None,
            None,
            None,
            QuotaStopStatusView::Dropped {
                at: STOPPED_AT + hour,
                reason: "the agent worked again before the reset".to_owned(),
            },
            "quota stop: 2026-11-09 14:00:00 +00:00 on the owner's sign-in; not resumed: the agent worked again before the reset\n",
        ),
    ];
    for (account, resets_at, switched_at, status, line) in cases {
        let stop = QuotaStopView {
            stopped_at: STOPPED_AT,
            account: account.map(str::to_owned),
            resets_at,
            switched_at,
            status,
        };
        let shown = shown_quota_stop(stop).await;
        assert!(shown.contains(line), "expected {line:?} in:\n{shown}");
    }
}

#[tokio::test]
async fn session_show_prints_the_chosen_compaction_threshold() {
    let home = Home::new();
    let mut shown = view("s0123456789ab", SessionStateView::Running);
    shown.compaction_threshold = Some("75".to_owned());
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Session {
            session: Box::new(shown),
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "show", "s0123456789ab"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s0123456789ab\trunning\tclaude\tstart\tc-1\t/p/app\t-\t-\t-\ncompaction threshold: 75\n"
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
async fn session_show_prints_how_the_agent_exited_and_its_last_lines() {
    let home = Home::new();
    let mut ended = view("s000000000002", SessionStateView::Ended);
    ended.reason = Some("its agent exited with status 1; resume reopens it".to_owned());
    ended.exit = Some(AgentExitView {
        status: ExitStatusView::Code { code: 1 },
        output: vec!["starting".to_owned(), "Error: refusing to start".to_owned()],
    });
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Session {
            session: Box::new(ended),
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "show", "s000000000002"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s000000000002\tended\tclaude\tstart\tc-1\t/p/app\t\
         its agent exited with status 1; resume reopens it \
         (last output: Error: refusing to start)\t-\t-\n\
         exit: status 1\n\
         output:\n  starting\n  Error: refusing to start\n\
         resume: torlek --profile dev session resume s000000000002\n"
    );
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionShow {
            session: "s000000000002".to_owned(),
        }
    );
}

#[tokio::test]
async fn session_show_of_an_ended_session_prints_how_to_resume_it() {
    let home = Home::new();
    let mut ended = view("s000000000002", SessionStateView::Ended);
    ended.reason = Some("its agent exited; resume reopens it".to_owned());
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Session {
            session: Box::new(ended),
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "show", "s000000000002"])
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s000000000002\tended\tclaude\tstart\tc-1\t/p/app\t\
         its agent exited; resume reopens it\t-\t-\n\
         resume: torlek --profile dev session resume s000000000002\n"
    );
}

#[tokio::test]
async fn session_show_of_an_ended_session_without_a_conversation_points_to_a_new_session() {
    let home = Home::new();
    let mut ended = view("s000000000002", SessionStateView::Failed);
    ended.conversation = None;
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Session {
            session: Box::new(ended),
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "show", "s000000000002"])
        .await;

    assert!(
        text(&output.stdout)
            .ends_with("no conversation to resume: torlek --profile dev session new /p/app\n"),
        "{}",
        text(&output.stdout)
    );
}

#[tokio::test]
async fn session_show_of_a_running_session_prints_only_its_line() {
    let home = Home::new();
    let _daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::Session {
            session: Box::new(view("s000000000001", SessionStateView::Running)),
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "show", "s000000000001"])
        .await;

    assert_eq!(
        text(&output.stdout),
        "s000000000001\trunning\tclaude\tstart\tc-1\t/p/app\t-\t-\t-\n"
    );
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
        "torlek: s000000000002 not restarted: conflict: it cannot resume\n"
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
        "torlek: conflict: session s0123456789ab is running; wake needs asleep\n"
    );
}

#[tokio::test]
async fn malformed_session_arguments_exit_2_before_any_connection() {
    let home = Home::new();
    let cases: [&[&str]; 9] = [
        &["session"],
        &["session", "new"],
        &["session", "new", "app", "--agent"],
        &["session", "new", "app", "--clone", "a", "--clone", "b"],
        &[
            "session", "new", "app", "--effort", "low", "--effort", "max",
        ],
        &["session", "new", "app", "--compaction-threshold"],
        &[
            "session",
            "new",
            "app",
            "--compaction-threshold",
            "200k",
            "--compaction-threshold",
            "1M",
        ],
        &["session", "new", "app", "--sandbox", "x"],
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
                    compaction_threshold: None,
                    sign_in_needed: None,
                    quota_stop: None,
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
        "torlek: s000000000002 not ended: conflict: its agent did not stop\n"
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
            backend: "fixture-backend".to_owned(),
            engine: "fixture-backend 1.0 (abc1234)".to_owned(),
            client_identity: "cockpit-s0123456789ab-0a1b2c3d4e5f".to_owned(),
        },
        platform: "linux".to_owned(),
        rules: vec![SandboxRuleView {
            rule: "read-write /p/app".to_owned(),
            enforced_by: "fixture mount view".to_owned(),
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
        "sandbox: enforced by fixture-backend on linux\n\
         engine: fixture-backend 1.0 (abc1234)\n\
         launch identity: cockpit-s0123456789ab-0a1b2c3d4e5f\n\
         rule: read-write /p/app <- fixture mount view\n\
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
                "/usr/bin/fixture-backend".to_owned(),
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
            .ends_with("observed: landlock: yes\ncommand: /usr/bin/fixture-backend -- <agent>\n"),
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
            "the sandbox refuses `agents.sandbox = \"required\"`: no sandbox backend on this host",
        )),
    );

    let output = home
        .cockpit(home.dir.path(), &["sandbox", "show", "/p/app"])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "torlek: refused: the sandbox refuses `agents.sandbox = \"required\"`: no sandbox backend on this host\n"
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
                "--max-active",
                "4",
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
            all_projects: false,
            without_confirmation: true,
            per_hour: Some(3),
            max_active: Some(4),
        }
    );
}

#[tokio::test]
async fn session_grant_over_every_project_is_one_named_flag() {
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
                "observe-sessions",
                "--all-projects",
            ],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        daemon.await.unwrap(),
        Command::SessionGrant {
            session: "s0123456789ab".to_owned(),
            permission: "observe-sessions".to_owned(),
            all_projects: true,
            without_confirmation: false,
            per_hour: None,
            max_active: None,
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
                    scope: "project app".to_owned(),
                    per_hour: None,
                    max_active: None,
                    needs_confirmation: None,
                },
                CoordinatorGrantView {
                    permission: "start-sessions".to_owned(),
                    grant: 5,
                    scope: "everything".to_owned(),
                    per_hour: Some(10),
                    max_active: Some(8),
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
        "grant\tobserve-sessions\t4\t-\tproject app\n\
         grant\tstart-sessions\t5\tper-hour=10 max-active=8 confirmed\teverything\n\
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
    let cases: [&[&str]; 9] = [
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
        &[
            "session",
            "grant",
            "s0123456789ab",
            "start-sessions",
            "--max-active",
        ],
        &[
            "session",
            "grant",
            "s0123456789ab",
            "start-sessions",
            "--max-active",
            "-1",
        ],
        &[
            "session",
            "grant",
            "s0123456789ab",
            "start-sessions",
            "--max-active",
            "2",
            "--max-active",
            "3",
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

/// #897: a send that overtook coordinator messages still held names each, oldest first.
#[tokio::test]
async fn session_send_names_the_held_messages_it_overtook() {
    let home = Home::new();
    let held = |message, from: &str| HeldMessageView {
        message,
        from: from.to_owned(),
    };
    let daemon = scripted_daemon(
        &home.daemon_socket(),
        Outcome::Ok(Reply::SessionAction {
            session: "s0123456789ab".to_owned(),
            held: vec![held(223, "sf27f63e06fde"), held(224, "s1111111111aa")],
        }),
    );

    let output = home
        .cockpit(home.dir.path(), &["session", "send", "s0123456789ab", "go"])
        .await;
    daemon.await.unwrap();

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "delivered to session s0123456789ab\n\
         still held, sent before it: message 223 from session sf27f63e06fde; it reaches the \
         agent when its turn ends, and the agent was told it waits\n\
         still held, sent before it: message 224 from session s1111111111aa; it reaches the \
         agent when its turn ends, and the agent was told it waits\n"
    );
}

#[tokio::test]
async fn session_send_and_cancel_preserve_the_message_and_acknowledge_delivery() {
    let id = "s0123456789ab";
    let message = "hello\nworld ; $(false)";
    for (args, expected) in [
        (
            vec!["session", "send", id, message],
            Command::SessionSend {
                session: id.into(),
                message: message.into(),
            },
        ),
        (
            vec!["session", "cancel", id],
            Command::SessionCancel { session: id.into() },
        ),
    ] {
        let home = Home::new();
        let daemon = scripted_daemon(
            &home.daemon_socket(),
            Outcome::Ok(Reply::SessionAction {
                session: id.into(),
                held: Vec::new(),
            }),
        );
        let output = home.cockpit(home.dir.path(), &args).await;
        assert!(output.status.success(), "{}", text(&output.stderr));
        assert_eq!(text(&output.stdout), format!("delivered to session {id}\n"));
        assert_eq!(daemon.await.unwrap(), expected);
    }
}
