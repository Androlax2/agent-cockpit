//! Issue #174: status polls read first and take the writer only when they must. A poll that
//! carries a relaunched agent's unspent token still binds it, through the agent socket.
#![cfg(target_os = "linux")]

mod common;

use cockpit_agent_channel::identity_of;
use cockpit_authorization::Permission;
use cockpit_protocol::{Command, ErrorCode, Reply, RunStateView};
use common::{GENEROUS, TestHome, refused};

#[tokio::test(flavor = "multi_thread")]
async fn a_relaunched_agents_first_status_poll_binds_its_new_token() {
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
    let second = home.launch("s1", &this_process, None).await;
    let _daemon = home.start();
    let agent = home.connect_agent().await;
    let status = |launch_token: Option<String>| Command::AskRunStatus {
        launch_token,
        request: request.id,
    };

    let before_binding = refused(agent.request(status(None), GENEROUS).await);
    let polled = agent
        .request(status(Some(second.as_str().to_owned())), GENEROUS)
        .await
        .unwrap();
    let after_binding = agent.request(status(None), GENEROUS).await.unwrap();

    assert_eq!(before_binding.code, ErrorCode::UnidentifiedPeer);
    let Reply::RunRequest { request: seen } = polled else {
        panic!("expected the run request, got {polled:?}");
    };
    assert_eq!((seen.id, seen.state), (request.id, RunStateView::Asking));
    assert!(matches!(after_binding, Reply::RunRequest { .. }));
}
