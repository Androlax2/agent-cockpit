//! `cockpit agent report` and `cockpit agent reports` driven as processes against a scripted
//! daemon socket. The crate graph keeps frontends from building the daemon, so the daemon's
//! side of these commands is proven in `cockpit-daemon/tests/agent_channel.rs`.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Output;

use cockpit_protocol::{
    AgentReport, AgentState, Command, ErrorCode, HandshakeReply, Outcome, ProtocolError,
    ProtocolVersion, Reply, Request, Response, ServerFrame, SessionReport, Unblock, UnblockedBy,
    Welcome,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

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

    /// Where the daemon puts a socket, `relative` to its socket directory.
    fn socket(&self, relative: &str) -> PathBuf {
        let path = self.runtime_dir().join("cockpit").join(relative);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    /// Runs `cockpit` with `arguments` in this home, off the test's runtime threads.
    async fn cockpit(&self, arguments: &[&str], variables: &[(&str, &Path)]) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_cockpit"));
        command
            .args(arguments)
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.runtime_dir());
        for (name, value) in variables {
            command.env(name, value);
        }
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

/// Accepts one connection on `socket`, answers the handshake, then answers its one request
/// with `outcome`, and returns the command it received.
fn scripted_daemon(socket: &Path, outcome: Outcome) -> tokio::task::JoinHandle<Command> {
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut lines = BufReader::new(read_half).lines();
        lines.next_line().await.unwrap().expect("a hello");
        let welcome = HandshakeReply::Accepted(Welcome {
            protocol_version: ProtocolVersion(1),
            capabilities: Vec::new(),
        });
        send(&mut write_half, serde_json::to_value(welcome).unwrap()).await;
        let request: Request =
            serde_json::from_str(&lines.next_line().await.unwrap().expect("a request")).unwrap();
        let response = ServerFrame::Response(Response {
            id: request.id,
            outcome,
        });
        send(&mut write_half, serde_json::to_value(response).unwrap()).await;
        request.command
    })
}

async fn send(writer: &mut tokio::net::unix::OwnedWriteHalf, frame: serde_json::Value) {
    let mut line = serde_json::to_vec(&frame).unwrap();
    line.push(b'\n');
    writer.write_all(&line).await.unwrap();
}

fn session_report(session: &str) -> SessionReport {
    SessionReport {
        session: session.to_owned(),
        state: Some(AgentState::Waiting),
        label: Some("Agent channel issue".to_owned()),
        doing: None,
        needs_input: Some("Start now?".to_owned()),
        reported_at: 1,
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn a_report_goes_to_the_agent_socket_with_the_launch_token() {
    let home = Home::new();
    let socket = home.socket("agent/agent.sock");
    let daemon = scripted_daemon(
        &socket,
        Outcome::Ok(Reply::AgentReported {
            report: session_report("s1"),
        }),
    );

    let output = home
        .cockpit(
            &[
                "agent",
                "report",
                "--doing",
                "writing the tests",
                "--state",
                "working",
            ],
            &[("COCKPIT_LAUNCH_TOKEN", Path::new("the-token"))],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "reported for session s1\n");
    assert_eq!(
        daemon.await.unwrap(),
        Command::AgentReport {
            launch_token: Some("the-token".to_owned()),
            report: AgentReport {
                state: Some(AgentState::Working),
                doing: Some("writing the tests".to_owned()),
                ..AgentReport::default()
            },
        }
    );
}

#[tokio::test]
async fn without_a_launch_token_the_report_carries_none() {
    let home = Home::new();
    let socket = home.socket("elsewhere/agent.sock");
    let daemon = scripted_daemon(
        &socket,
        Outcome::Ok(Reply::AgentReported {
            report: session_report("s1"),
        }),
    );

    let output = home
        .cockpit(
            &["agent", "report", "--label", "Agent channel issue"],
            &[("COCKPIT_AGENT_SOCKET", &socket)],
        )
        .await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    let Command::AgentReport { launch_token, .. } = daemon.await.unwrap() else {
        panic!("expected an agent report");
    };
    assert_eq!(launch_token, None);
}

#[tokio::test]
async fn a_refusal_exits_1_saying_what_unblocks_it() {
    let home = Home::new();
    let socket = home.socket("agent/agent.sock");
    let _daemon = scripted_daemon(
        &socket,
        Outcome::Error(ProtocolError {
            code: ErrorCode::NotAuthorized,
            message: "the session holds no change-state grant for this".to_owned(),
            unblock: Some(Unblock {
                action: "the owner grants change-state over session s1".to_owned(),
                by: UnblockedBy::Owner,
            }),
        }),
    );

    let output = home
        .cockpit(&["agent", "report", "--doing", "writing the tests"], &[])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        text(&output.stderr),
        "cockpit: refused: the session holds no change-state grant for this\n\
         cockpit: to unblock (the owner): the owner grants change-state over session s1\n"
    );
}

#[tokio::test]
async fn the_owner_lists_every_sessions_report_from_the_host_only_socket() {
    let home = Home::new();
    let socket = home.socket("daemon.sock");
    let daemon = scripted_daemon(
        &socket,
        Outcome::Ok(Reply::AgentReports {
            reports: vec![session_report("s1")],
        }),
    );

    let output = home.cockpit(&["agent", "reports"], &[]).await;

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        text(&output.stdout),
        "s1 waiting label=\"Agent channel issue\" needs-input=\"Start now?\"\n"
    );
    assert_eq!(daemon.await.unwrap(), Command::AgentReports);
}

#[tokio::test]
async fn an_unreachable_daemon_exits_1_naming_the_socket() {
    let home = Home::new();
    let socket = home.socket("agent/agent.sock");

    let output = home
        .cockpit(&["agent", "report", "--doing", "writing the tests"], &[])
        .await;

    assert_eq!(output.status.code(), Some(1));
    assert!(
        text(&output.stderr).contains(&socket.display().to_string()),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn malformed_report_arguments_exit_2_before_any_connection() {
    let home = Home::new();
    let cases: [&[&str]; 5] = [
        &["agent", "report"],
        &["agent", "report", "--state", "asleep"],
        &["agent", "report", "--doing"],
        &["agent", "report", "--doing", "a b", "--doing", "c d"],
        &["agent", "report", "--session", "s2"],
    ];
    for arguments in cases {
        let output = home.cockpit(arguments, &[]).await;

        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        assert!(
            text(&output.stderr).contains("usage:"),
            "{arguments:?}: {}",
            text(&output.stderr)
        );
    }
}
