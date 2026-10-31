//! Issue #174: status polls read first and take the writer only when they must. A poll that
//! carries a relaunched agent's unspent token still binds it, through the agent socket.
#![cfg(target_os = "linux")]

mod common;

use cockpit_agent_channel::{LaunchToken, identity_of};
use cockpit_authorization::Permission;
use cockpit_client::Connection;
use cockpit_protocol::{Command, ErrorCode, Reply, RunStateView};
use common::{GENEROUS, RunningDaemon, TestHome, refused};

/// An agent asked to run, then was relaunched while the daemon was down: its new token is
/// pending and its request still waits for the owner.
struct Relaunched {
    _home: TestHome,
    _daemon: RunningDaemon,
    agent: Connection,
    token: LaunchToken,
    request: i64,
}

async fn relaunched() -> Relaunched {
    let home = TestHome::new();
    let this_process = identity_of(std::process::id()).unwrap();
    let first = home
        .launch("s1", &this_process, Some(Permission::RunCommands))
        .await;
    let daemon = home.start();
    let agent = home.connect_agent().await;
    let cwd = std::fs::canonicalize(home.home()).unwrap();
    let Reply::RunRequest { request } = agent
        .request(
            Command::AskRun {
                launch_token: Some(first.as_str().to_owned()),
                command: "true".to_owned(),
                cwd: cwd.to_str().unwrap().to_owned(),
                reason: "Waits for the owner across a relaunch.".to_owned(),
            },
            GENEROUS,
        )
        .await
        .unwrap()
    else {
        panic!("expected a run request");
    };
    daemon.kill();
    let token = home.launch("s1", &this_process, None).await;
    let daemon = home.start();
    let agent = home.connect_agent().await;
    Relaunched {
        _home: home,
        _daemon: daemon,
        agent,
        token,
        request: request.id,
    }
}

impl Relaunched {
    async fn poll(
        &self,
        launch_token: Option<&LaunchToken>,
    ) -> Result<Reply, cockpit_client::ClientError> {
        let command = Command::AskRunStatus {
            launch_token: launch_token.map(|token| token.as_str().to_owned()),
            request: self.request,
        };
        self.agent.request(command, GENEROUS).await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relaunched_agent_is_unidentified_until_its_new_token_binds() {
    let relaunched = relaunched().await;

    let polled = refused(relaunched.poll(None).await);

    assert_eq!(polled.code, ErrorCode::UnidentifiedPeer);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relaunched_agents_first_status_poll_binds_its_new_token() {
    let relaunched = relaunched().await;

    let polled = relaunched.poll(Some(&relaunched.token)).await.unwrap();

    let Reply::RunRequest { request } = polled else {
        panic!("expected the run request, got {polled:?}");
    };
    assert_eq!(
        (request.id, request.state),
        (relaunched.request, RunStateView::Asking)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn after_the_binding_poll_the_tree_alone_identifies_the_agent() {
    let relaunched = relaunched().await;
    relaunched.poll(Some(&relaunched.token)).await.unwrap();

    let polled = relaunched.poll(None).await.unwrap();

    assert!(matches!(polled, Reply::RunRequest { .. }), "got {polled:?}");
}
