//! Issue #706 at the daemon's socket: the change feed carries a `view-stamp` for each part of
//! the owner's view that is answered by request, and a commit that changes what a part shows
//! moves that part's stamp, so the TUI asks again at once instead of polling.
#![cfg(target_os = "linux")]

mod candidate;
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::process::Command as Process;
use std::time::{Duration, Instant};

use cockpit_agent_channel::identity_of;
use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_client::{Connection, Subscription};
use cockpit_protocol::{
    AgentReport, AgentState, Change, Command, EntityKind, Reply, ViewArea, ViewStamp,
};
use common::{GENEROUS, TestHome};

/// A running daemon, the owner's connection and a feed that has seen every area's first stamp.
struct Watched {
    home: TestHome,
    /// The launch token of a session that may run commands and change its own state.
    token: String,
    _daemon: common::RunningDaemon,
    owner: Connection,
    feed: Subscription,
    stamps: BTreeMap<ViewArea, u64>,
}

impl Watched {
    async fn start() -> Watched {
        let home = TestHome::new();
        let this_process = identity_of(std::process::id()).unwrap();
        let token = home
            .launch("s1", &this_process, Some(Permission::RunCommands))
            .await
            .as_str()
            .to_owned();
        // Closed again before the daemon opens the state.
        {
            let session = SessionId::new("s1").unwrap();
            let database = home.open_state().await;
            let write = database.write().await.unwrap();
            cockpit_authorization::grant(
                &write,
                &session,
                Permission::ChangeState,
                &Scope::Session(session.clone()),
            )
            .await
            .unwrap();
            write.commit().await.unwrap();
        }
        let daemon = home.start();
        let owner = home.connect().await;
        let feed = Subscription::new(home.socket_path(), None, GENEROUS);
        let mut watched = Watched {
            home,
            token,
            _daemon: daemon,
            owner,
            feed,
            stamps: BTreeMap::new(),
        };
        while watched.stamps.len() < ViewArea::ALL.len() {
            watched.next_moved().await;
        }
        watched
    }

    /// Applies the next batch and returns the areas whose stamp it moved.
    async fn next_moved(&mut self) -> BTreeSet<ViewArea> {
        let pending = tokio::time::timeout(GENEROUS, self.feed.next_batch())
            .await
            .expect("the feed sends a batch")
            .expect("the feed continues");
        let mut moved = BTreeSet::new();
        for event in &pending.batch().changes {
            let (EntityKind::ViewStamp, Change::Upsert { state }) =
                (&event.entity.kind, &event.change)
            else {
                continue;
            };
            let area = ViewArea::from_id(&event.entity.id).expect("a known area");
            let stamp: ViewStamp = serde_json::from_value(state.clone().into()).unwrap();
            if self.stamps.insert(area, stamp.changed_at) != Some(stamp.changed_at) {
                moved.insert(area);
            }
        }
        pending.applied();
        moved
    }

    /// Everything that moved until `area` did.
    async fn moved_until(&mut self, area: ViewArea) -> BTreeSet<ViewArea> {
        let deadline = Instant::now() + GENEROUS;
        let mut moved = BTreeSet::new();
        while !moved.contains(&area) {
            assert!(Instant::now() < deadline, "{area:?} never moved: {moved:?}");
            moved.extend(self.next_moved().await);
        }
        moved
    }

    fn project_root(&self) -> std::path::PathBuf {
        self.home.home().join("project")
    }

    async fn add_project(&self) {
        let root = self.project_root();
        std::fs::create_dir_all(&root).unwrap();
        for arguments in [
            vec!["init", "--quiet"],
            vec!["commit", "--quiet", "--allow-empty", "-m", "fixture"],
        ] {
            let done = Process::new("git")
                .current_dir(&root)
                .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.com"])
                .args(arguments)
                .output()
                .unwrap();
            assert!(done.status.success(), "{done:?}");
        }
        self.owner
            .request(
                Command::ProjectAdd {
                    path: root.display().to_string(),
                    name: Some("project".to_owned()),
                },
                GENEROUS,
            )
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn every_area_has_a_stamp_from_the_first_snapshot() {
    let watched = Watched::start().await;

    assert_eq!(
        watched.stamps.keys().copied().collect::<Vec<_>>(),
        ViewArea::ALL
    );
}

#[tokio::test]
async fn adding_a_project_moves_the_projects_stamp_only() {
    let mut watched = Watched::start().await;

    watched.add_project().await;

    let moved = watched.moved_until(ViewArea::Projects).await;
    assert_eq!(moved, BTreeSet::from([ViewArea::Projects]));
}

#[tokio::test]
async fn filing_a_task_moves_the_tasks_stamp() {
    let mut watched = Watched::start().await;
    watched.add_project().await;
    watched.moved_until(ViewArea::Projects).await;

    watched
        .owner
        .request(
            Command::TaskAdd {
                launch_token: None,
                project: Some("project".to_owned()),
                title: "Fix login".to_owned(),
                notes: String::new(),
                item_kind: None,
                priority: None,
                after: Vec::new(),
            },
            GENEROUS,
        )
        .await
        .unwrap();

    watched.moved_until(ViewArea::Tasks).await;
}

#[tokio::test]
async fn an_agent_report_moves_the_reports_stamp() {
    let mut watched = Watched::start().await;
    let agent = watched.home.connect_agent().await;

    agent
        .request(
            Command::AgentReport {
                launch_token: Some(watched.token.clone()),
                report: AgentReport {
                    state: Some(AgentState::Waiting),
                    ..AgentReport::default()
                },
            },
            GENEROUS,
        )
        .await
        .unwrap();

    watched.moved_until(ViewArea::AgentReports).await;
}

#[tokio::test]
async fn a_request_to_run_a_command_moves_the_approvals_stamp() {
    let mut watched = Watched::start().await;
    let agent = watched.home.connect_agent().await;
    let cwd = std::fs::canonicalize(watched.home.home()).unwrap();

    let Reply::RunRequest { .. } = agent
        .request(
            Command::AskRun {
                launch_token: Some(watched.token.clone()),
                command: "true".to_owned(),
                cwd: cwd.to_str().unwrap().to_owned(),
                reason: "Moves the approvals stamp.".to_owned(),
                writes: Vec::new(),
                hosts: Vec::new(),
            },
            GENEROUS,
        )
        .await
        .unwrap()
    else {
        panic!("expected a run request");
    };

    watched.moved_until(ViewArea::Approvals).await;
}

#[tokio::test]
async fn an_idle_daemon_moves_no_stamp() {
    let mut watched = Watched::start().await;

    let quiet = tokio::time::timeout(Duration::from_secs(2), watched.next_moved()).await;

    assert!(quiet.is_err(), "a stamp moved with nothing committed");
}
