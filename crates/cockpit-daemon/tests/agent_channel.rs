//! Issue #32's acceptance through the daemon binary's sockets: the sender's identity comes
//! from its launch token and verified process tree, never from a payload, and nested agents
//! act with their parent session's grants. This test process plays the launched agent.
#![cfg(target_os = "linux")]

mod common;

use std::process::{Command as Process, Stdio};

use cockpit_agent_channel::{LaunchToken, ProcessIdentity, identity_of};
use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_client::ClientError;
use cockpit_protocol::{
    AgentReport, Command, ErrorCode, ProtocolError, Reply, SessionReport, UnblockedBy,
};
use common::{GENEROUS, TestHome};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Set in the child this file spawns as a nested agent; see `nested_agent`.
const NESTED_AGENT_SOCKET: &str = "COCKPIT_TEST_NESTED_AGENT_SOCKET";

fn session(id: &str) -> SessionId {
    SessionId::new(id).unwrap()
}

fn this_process() -> ProcessIdentity {
    identity_of(std::process::id()).unwrap()
}

/// Launches `session` rooted at `root`, optionally granting it `change-state` over itself,
/// while no daemon runs.
async fn launch(home: &TestHome, id: &str, root: &ProcessIdentity, granted: bool) -> LaunchToken {
    let database = home.open_state().await;
    let write = database.write().await.unwrap();
    let token = cockpit_agent_channel::mint(&write, &session(id))
        .await
        .unwrap();
    cockpit_agent_channel::record_root(&write, &session(id), root)
        .await
        .unwrap();
    if granted {
        cockpit_authorization::grant(
            &write,
            &session(id),
            Permission::ChangeState,
            &Scope::Session(session(id)),
        )
        .await
        .unwrap();
    }
    write.commit().await.unwrap();
    token
}

fn doing(text: &str) -> AgentReport {
    AgentReport {
        doing: Some(text.to_owned()),
        ..AgentReport::default()
    }
}

fn report_command(token: Option<&LaunchToken>, report: AgentReport) -> Command {
    Command::AgentReport {
        launch_token: token.map(|token| token.as_str().to_owned()),
        report,
    }
}

async fn owner_view(home: &TestHome) -> Vec<SessionReport> {
    match home
        .connect()
        .await
        .request(Command::AgentReports, GENEROUS)
        .await
    {
        Ok(Reply::AgentReports { reports }) => reports,
        other => panic!("expected the reports, got {other:?}"),
    }
}

fn refused(result: Result<Reply, ClientError>) -> ProtocolError {
    match result {
        Err(ClientError::Daemon(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_launched_agent_reports_and_the_owner_reads_it() {
    let home = TestHome::new();
    let token = launch(&home, "s1", &this_process(), true).await;
    let _daemon = home.start();

    let reply = home
        .connect_agent()
        .await
        .request(
            report_command(Some(&token), doing("writing the tests")),
            GENEROUS,
        )
        .await;

    let Ok(Reply::AgentReported { report }) = reply else {
        panic!("expected the recorded report, got {reply:?}");
    };
    assert_eq!(report.session, "s1");
    let reports = owner_view(&home).await;
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].doing.as_deref(), Some("writing the tests"));
}

#[tokio::test]
async fn a_token_leaked_to_another_process_tree_is_refused() {
    let home = TestHome::new();
    let mut agent = Process::new("sleep")
        .arg("60")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let token = launch(&home, "s1", &identity_of(agent.id()).unwrap(), true).await;
    let _daemon = home.start();

    let refusal = refused(
        home.connect_agent()
            .await
            .request(
                report_command(Some(&token), doing("pretending to be s1")),
                GENEROUS,
            )
            .await,
    );

    agent.kill().unwrap();
    agent.wait().unwrap();
    assert_eq!(refusal.code, ErrorCode::UnidentifiedPeer);
    assert_eq!(refusal.unblock.unwrap().by, UnblockedBy::Agent);
    assert_eq!(owner_view(&home).await, Vec::new());
}

/// A raw frame, so the payload can carry fields no typed client would send.
#[tokio::test]
async fn a_session_named_in_the_payload_does_not_change_who_reports() {
    let home = TestHome::new();
    let token = launch(&home, "s1", &this_process(), true).await;
    launch(&home, "s2", &identity_of(1).unwrap(), true).await;
    let _daemon = home.start();
    let stream = tokio::net::UnixStream::connect(home.agent_socket_path())
        .await
        .unwrap();
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let request = serde_json::json!({
        "id": 1,
        "session": "s2",
        "command": {
            "kind": "agent-report",
            "session": "s2",
            "launch_token": token.as_str(),
            "report": {"session": "s2", "doing": "claiming to be s2"}
        }
    });

    write_half
        .write_all(format!("{{\"protocol_versions\":[1]}}\n{request}\n").as_bytes())
        .await
        .unwrap();
    lines.next_line().await.unwrap().expect("a handshake reply");
    let response = lines.next_line().await.unwrap().expect("a response");

    assert!(response.contains("\"session\":\"s1\""), "{response}");
    let reports = owner_view(&home).await;
    assert_eq!(
        reports
            .iter()
            .map(|report| report.session.as_str())
            .collect::<Vec<_>>(),
        ["s1"]
    );
}

#[tokio::test]
async fn a_nested_agent_reports_with_its_parent_sessions_grants() {
    let home = TestHome::new();
    let token = launch(&home, "s1", &this_process(), true).await;
    let _daemon = home.start();
    home.connect_agent()
        .await
        .request(
            report_command(Some(&token), doing("starting a subagent")),
            GENEROUS,
        )
        .await
        .unwrap();

    let nested = Process::new(std::env::current_exe().unwrap())
        .args(["--exact", "nested_agent", "--nocapture"])
        .env(NESTED_AGENT_SOCKET, home.agent_socket_path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();

    assert!(
        nested.status.success(),
        "{}",
        String::from_utf8_lossy(&nested.stderr)
    );
    let reports = owner_view(&home).await;
    assert_eq!(reports[0].session, "s1");
    assert_eq!(
        reports[0].doing.as_deref(),
        Some("reporting from the subagent")
    );
}

/// The nested agent: a child of the test above, sending no token. Outside that test it has
/// no socket and does nothing.
#[tokio::test]
async fn nested_agent() {
    let Some(socket) = std::env::var_os(NESTED_AGENT_SOCKET) else {
        return;
    };
    let connection = cockpit_client::Connection::open(socket.as_ref(), GENEROUS)
        .await
        .unwrap();

    let reply = connection
        .request(
            report_command(None, doing("reporting from the subagent")),
            GENEROUS,
        )
        .await;

    let Ok(Reply::AgentReported { report }) = reply else {
        panic!("the nested agent was refused: {reply:?}");
    };
    assert_eq!(report.session, "s1");
}

#[tokio::test]
async fn an_ungranted_session_is_refused_with_what_unblocks_it() {
    let home = TestHome::new();
    let token = launch(&home, "s1", &this_process(), false).await;
    let _daemon = home.start();

    let refusal = refused(
        home.connect_agent()
            .await
            .request(
                report_command(Some(&token), doing("writing the tests")),
                GENEROUS,
            )
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::NotAuthorized);
    let unblock = refusal.unblock.unwrap();
    assert_eq!(unblock.by, UnblockedBy::Owner);
    assert_eq!(
        unblock.action,
        "the owner grants change-state over session s1"
    );
}

#[tokio::test]
async fn an_unlaunched_process_is_unidentified() {
    let home = TestHome::new();
    let _daemon = home.start();

    let refusal = refused(
        home.connect_agent()
            .await
            .request(report_command(None, doing("writing the tests")), GENEROUS)
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::UnidentifiedPeer);
    assert_eq!(refusal.unblock.unwrap().by, UnblockedBy::Owner);
}

#[tokio::test]
async fn an_invalid_report_is_refused_naming_its_field() {
    let home = TestHome::new();
    let token = launch(&home, "s1", &this_process(), true).await;
    let _daemon = home.start();

    let refusal = refused(
        home.connect_agent()
            .await
            .request(report_command(Some(&token), doing("one")), GENEROUS)
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::InvalidArgument);
    assert!(refusal.message.starts_with("doing:"), "{}", refusal.message);
}

#[tokio::test]
async fn the_agent_socket_refuses_the_owners_commands() {
    let home = TestHome::new();
    let _daemon = home.start();

    let refusal = refused(
        home.connect_agent()
            .await
            .request(Command::AgentReports, GENEROUS)
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::NotAuthorized);
    assert_eq!(refusal.unblock.unwrap().by, UnblockedBy::Owner);
}

#[tokio::test]
async fn the_host_only_socket_takes_no_agent_report() {
    let home = TestHome::new();
    let _daemon = home.start();

    let refusal = refused(
        home.connect()
            .await
            .request(report_command(None, doing("writing the tests")), GENEROUS)
            .await,
    );

    assert_eq!(refusal.code, ErrorCode::NotAuthorized);
    assert_eq!(refusal.unblock.unwrap().by, UnblockedBy::Agent);
}

#[tokio::test]
async fn a_bound_session_is_identified_after_a_daemon_restart() {
    let home = TestHome::new();
    let token = launch(&home, "s1", &this_process(), true).await;
    let daemon = home.start();
    home.connect_agent()
        .await
        .request(
            report_command(Some(&token), doing("before the restart")),
            GENEROUS,
        )
        .await
        .unwrap();
    daemon.kill();
    let _restarted = home.start();

    let reply = home
        .connect_agent()
        .await
        .request(report_command(None, doing("after the restart")), GENEROUS)
        .await;

    assert!(
        matches!(reply, Ok(Reply::AgentReported { .. })),
        "{reply:?}"
    );
}

#[tokio::test]
async fn the_agent_socket_has_a_private_directory_of_its_own() {
    use std::os::unix::fs::PermissionsExt;
    let home = TestHome::new();
    let _daemon = home.start();

    let agent_dir = home.agent_socket_path().parent().unwrap().to_path_buf();
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;

    assert_ne!(agent_dir, home.socket_dir());
    assert_eq!(mode(&agent_dir), 0o700);
    assert_eq!(mode(&home.agent_socket_path()), 0o600);
    let entries: Vec<_> = std::fs::read_dir(&agent_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["agent.sock"]);
}
