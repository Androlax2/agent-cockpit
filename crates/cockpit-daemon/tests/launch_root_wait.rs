//! Issue #279 at the agent socket: an agent's first request can arrive before the launcher
//! recorded its launch's process tree. The daemon holds it until the root is recorded,
//! instead of refusing it. This test process plays the agent, and records its own root
//! while its request waits, as the launcher does moments after the agent starts.
#![cfg(target_os = "linux")]

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use cockpit_agent_channel::{LaunchToken, identity_of};
use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_client::Connection;
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes};
use cockpit_protocol::{AgentReport, Command, Reply};
use cockpit_turso::Database;
use common::GENEROUS;

/// Long enough that the request would have been answered, were it not held.
const BEFORE_THE_ROOT: Duration = Duration::from_millis(300);

struct Running {
    daemon: Daemon,
    database: Arc<Database>,
    _home: tempfile::TempDir,
}

async fn start() -> Running {
    let home = tempfile::tempdir().unwrap();
    let kept = Arc::new(Mutex::new(None));
    let options = Options {
        runtime_dir: home.path().join("run/cockpit"),
        state_dir: home.path().join("state/cockpit"),
        limits: Limits::default(),
    };
    let daemon = Instance::acquire(options)
        .unwrap()
        .start(common::wiring_in(home.path(), Vec::new()), {
            let kept = Arc::clone(&kept);
            move |capabilities| {
                *kept.lock().unwrap() = Some(Arc::clone(&capabilities.database));
                Arc::new(Routes::new(
                    capabilities,
                    cockpit_config::Environment::default(),
                ))
            }
        })
        .await
        .unwrap();
    let database = kept.lock().unwrap().take().unwrap();
    Running {
        daemon,
        database,
        _home: home,
    }
}

/// Mints a launch of `session` granted `permission`, whose root is not recorded yet.
async fn launch_without_root(
    database: &Database,
    session: &str,
    permission: Permission,
) -> LaunchToken {
    let session = SessionId::new(session).unwrap();
    let write = database.write().await.unwrap();
    let token = cockpit_agent_channel::mint(&write, &session).await.unwrap();
    cockpit_authorization::grant(
        &write,
        &session,
        permission,
        &Scope::Session(session.clone()),
    )
    .await
    .unwrap();
    write.commit().await.unwrap();
    token
}

/// Records this test process as `session`'s launch root, as the launcher does.
async fn record_this_process_as_root(database: &Database, session: &str) {
    let session = SessionId::new(session).unwrap();
    let write = database.write().await.unwrap();
    cockpit_agent_channel::record_root(&write, &session, &identity_of(std::process::id()).unwrap())
        .await
        .unwrap();
    write.commit().await.unwrap();
}

/// Sends `command` on the agent socket, records the root once it has waited
/// [`BEFORE_THE_ROOT`] unanswered, and returns the answer.
async fn answer_once_the_root_lands(running: &Running, session: &str, command: Command) -> Reply {
    let agent = Connection::open(running.daemon.agent_socket_path(), GENEROUS)
        .await
        .unwrap();
    let request = tokio::spawn(async move { agent.request(command, GENEROUS).await });
    tokio::time::sleep(BEFORE_THE_ROOT).await;
    assert!(
        !request.is_finished(),
        "the request was answered before the launch's root was recorded: {:?}",
        request.await
    );
    record_this_process_as_root(&running.database, session).await;
    request.await.unwrap().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_report_before_the_launch_root_is_recorded_lands_once_it_is() {
    let running = start().await;
    let token = launch_without_root(&running.database, "s1", Permission::ChangeState).await;
    let report = Command::AgentReport {
        launch_token: Some(token.as_str().to_owned()),
        report: AgentReport {
            label: Some("the first report".to_owned()),
            ..AgentReport::default()
        },
    };

    let reply = answer_once_the_root_lands(&running, "s1", report).await;

    let Reply::AgentReported { report } = reply else {
        panic!("expected the recorded report, got {reply:?}");
    };
    assert_eq!(report.session, "s1");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_check_before_the_launch_root_is_recorded_is_answered_once_it_is() {
    let running = start().await;
    let token = launch_without_root(&running.database, "s1", Permission::RunCommands).await;
    let check = Command::AskCheck {
        launch_token: Some(token.as_str().to_owned()),
    };

    let reply = answer_once_the_root_lands(&running, "s1", check).await;

    assert_eq!(
        reply,
        Reply::AskReady {
            session: "s1".to_owned()
        }
    );
}
