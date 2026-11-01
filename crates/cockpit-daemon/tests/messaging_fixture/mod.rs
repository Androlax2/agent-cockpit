#![allow(dead_code, reason = "shared by the messaging transport fixtures")]
use crate::common::GENEROUS;
use cockpit_client::Connection;
use cockpit_daemon::{Daemon, Instance, Limits, Options, Routes, Wiring};
use cockpit_protocol::{Command, Reply, SessionView};
use cockpit_provider_api::Provider;
use cockpit_provider_api::fake::FakeProvider;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Home {
    pub dir: tempfile::TempDir,
    pub database: Mutex<Option<Arc<cockpit_turso::Database>>>,
}
impl Home {
    pub fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            database: Mutex::new(None),
        }
    }
    pub fn path(&self) -> &Path {
        self.dir.path()
    }
    pub fn options(&self) -> Options {
        Options {
            runtime_dir: self.path().join("run"),
            state_dir: self.path().join("state"),
            limits: Limits::default(),
        }
    }
    pub async fn start(&self, providers: Vec<Arc<dyn Provider>>) -> Daemon {
        self.start_wired(crate::common::wiring_in(self.path(), providers))
            .await
    }
    pub async fn start_wired(&self, wiring: Wiring) -> Daemon {
        let deadline = Instant::now() + GENEROUS;
        let instance = loop {
            match Instance::acquire(self.options()) {
                Ok(instance) => break instance,
                Err(error) => {
                    assert!(Instant::now() < deadline, "{error}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        };
        instance
            .start(wiring, |capabilities| {
                *self.database.lock().unwrap() = Some(Arc::clone(&capabilities.database));
                Arc::new(Routes::new(
                    capabilities,
                    cockpit_config::Environment {
                        home: Some(self.path().join("home").into()),
                        ..Default::default()
                    },
                ))
            })
            .await
            .unwrap()
    }
    pub async fn connect(&self) -> Connection {
        Connection::open(&self.options().socket_path(), GENEROUS)
            .await
            .unwrap()
    }
    pub async fn project(&self, client: &Connection) -> PathBuf {
        let root = self.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "--quiet"],
            vec!["commit", "--quiet", "--allow-empty", "-m", "fixture"],
        ] {
            let out = Process::new("git")
                .current_dir(&root)
                .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.com"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "{out:?}");
        }
        client
            .request(
                Command::ProjectAdd {
                    path: root.display().to_string(),
                    name: None,
                },
                GENEROUS,
            )
            .await
            .unwrap();
        root
    }
    pub async fn session(&self, client: &Connection, root: &Path, agent: &str) -> SessionView {
        let reply = client
            .request(
                Command::SessionNew {
                    dir: root.display().to_string(),
                    agent: Some(agent.into()),
                    prompt: None,
                    clone: None,
                },
                GENEROUS,
            )
            .await
            .unwrap();
        let Reply::Session { session } = reply else {
            panic!("{reply:?}")
        };
        *session
    }
    pub async fn waiting(&self, session: &str) {
        let launches =
            crate::common::once_logged(1, || FakeProvider::launches(&self.path().join("launches")))
                .await;
        let launch = &launches[0];
        assert_eq!(launch.session, session);
        let database = self.database.lock().unwrap().clone().unwrap();
        let write = database.write().await.unwrap();
        cockpit_sessions::report(
            &write,
            &cockpit_agent_channel::ancestry(launch.pid as u32),
            Some(&launch.token),
            cockpit_sessions::Report {
                state: Some(cockpit_sessions::AgentState::Waiting),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
    }
    pub async fn messages(&self, count: usize) -> Vec<(String, String)> {
        crate::common::once_logged(count, || {
            FakeProvider::messages(&self.path().join("messages"))
        })
        .await
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        let _ = Process::new("tmux")
            .arg("-S")
            .arg(self.path().join("tmux.sock"))
            .arg("kill-server")
            .env_remove("TMUX")
            .output();
    }
}
