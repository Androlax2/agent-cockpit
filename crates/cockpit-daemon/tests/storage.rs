//! Storage reporting is owner-only and must refuse agents before filesystem work.

mod candidate;
mod common;

use cockpit_client::ClientError;
#[cfg(target_os = "linux")]
use cockpit_protocol::UnblockedBy;
use cockpit_protocol::{Command, ErrorCode};
use common::GENEROUS;
#[cfg(target_os = "linux")]
use common::TestHome;

#[tokio::test]
#[cfg(target_os = "linux")]
async fn storage_refuses_the_agent_socket() {
    let home = TestHome::new();
    let _daemon = home.start();
    let agent = home.connect_agent().await;
    let refusal = match agent.request(Command::StorageReport, GENEROUS).await {
        Err(ClientError::Daemon(error)) => error,
        other => panic!("expected owner-only refusal, got {other:?}"),
    };
    assert_eq!(refusal.code, ErrorCode::NotAuthorized);
    assert_eq!(
        refusal.unblock.map(|unblock| unblock.by),
        Some(UnblockedBy::Owner)
    );
}

#[tokio::test]
async fn storage_uses_bulk_admission_and_leaves_control_capacity() {
    use cockpit_client::Connection;
    use cockpit_daemon::{
        Handler, Instance, LaneLimits, Limits, Options, Peer, Route, Routes, WorkClass,
    };
    use cockpit_protocol::{FeedPosition, Outcome, Reply, Welcome};
    use std::sync::Arc;
    use tokio::sync::{Notify, Semaphore};

    struct HeldBulk {
        routes: Routes,
        started: Arc<Notify>,
        release: Arc<Semaphore>,
    }
    impl Handler for HeldBulk {
        fn route(&self, command: Command, welcome: &Welcome, peer: Peer) -> Route {
            if matches!(command, Command::Subscribe { .. }) {
                let started = Arc::clone(&self.started);
                let release = Arc::clone(&self.release);
                return Route::Run {
                    class: WorkClass::Bulk,
                    job: Box::pin(async move {
                        started.notify_one();
                        let _permit = release.acquire().await.unwrap();
                        Outcome::Ok(Reply::Pong)
                    }),
                };
            }
            self.routes.route(command, welcome, peer)
        }
    }
    let home = tempfile::tempdir().unwrap();
    let root = home.path().canonicalize().unwrap();
    let started = Arc::new(Notify::new());
    let release = Arc::new(Semaphore::new(0));
    let daemon = Instance::acquire(Options {
        runtime_dir: root.join("run"),
        state_dir: root.join("state"),
        limits: Limits {
            bulk: LaneLimits {
                workers: 1,
                waiting: 0,
            },
            ..Limits::default()
        },
    })
    .unwrap()
    .start(common::wiring_in(&root, Vec::new()), |capabilities| {
        Arc::new(HeldBulk {
            routes: Routes::new(
                capabilities,
                cockpit_config::Environment {
                    home: Some(root.join("home").into()),
                    ..cockpit_config::Environment::default()
                },
            ),
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        })
    })
    .await
    .unwrap();
    let client = Arc::new(
        Connection::open(daemon.socket_path(), GENEROUS)
            .await
            .unwrap(),
    );
    let held_client = Arc::clone(&client);
    let held = tokio::spawn(async move {
        held_client
            .request(
                Command::Subscribe {
                    from: FeedPosition::Snapshot,
                },
                GENEROUS,
            )
            .await
    });
    tokio::time::timeout(GENEROUS, started.notified())
        .await
        .unwrap();
    let storage = client.request(Command::StorageReport, GENEROUS).await;
    let ping = client.request(Command::Ping, GENEROUS).await;
    release.add_permits(1);
    held.await.unwrap().unwrap();
    assert!(
        matches!(storage, Err(ClientError::Daemon(ref error)) if error.code == ErrorCode::Busy && error.message.contains("bulk")),
        "{storage:?}"
    );
    assert_eq!(ping.unwrap(), Reply::Pong);
    assert_eq!(
        client
            .request(Command::StorageReport, GENEROUS)
            .await
            .unwrap(),
        Reply::Storage {
            clones: Vec::new(),
            truncated: false,
            end_sweeps: Vec::new(),
            session_temps: Some(Vec::new()),
            session_temps_truncated: false,
            session_temps_unavailable: None,
        }
    );
    drop(daemon);
}
