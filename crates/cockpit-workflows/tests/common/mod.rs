//! A scratch state database with the authorization, tasks and workflows tables.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::path::{Path, PathBuf};

use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_provider_api::{Capabilities, Capability, Provenance};
use cockpit_tasks::{Item, ItemId, Kind, NewItem, Priority};
use cockpit_turso::Database;
use cockpit_workflows::{Actor, Completion, Handoff, Preset, Refusal, Run, RunId, select_preset};

pub const NOW: i64 = 1_800_000_000;

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub database: Database,
}

pub fn migrations() -> [cockpit_turso::MigrationSet; 3] {
    [
        cockpit_authorization::MIGRATIONS,
        cockpit_tasks::MIGRATIONS,
        cockpit_workflows::MIGRATIONS,
    ]
}

impl Fixture {
    pub async fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&state_path(dir.path()), &migrations())
            .await
            .unwrap();
        Fixture { dir, database }
    }

    /// The same database, closed and opened again as a restarted daemon opens it.
    pub async fn reopen(self) -> Fixture {
        let Fixture { dir, database } = self;
        drop(database);
        let database = Database::open(&state_path(dir.path()), &migrations())
            .await
            .unwrap();
        Fixture { dir, database }
    }

    /// A session of `project`, holding `change-state` over itself as launched sessions do.
    pub async fn session(&self, id: &str, project: &str) -> Actor {
        let session = SessionId::new(id).unwrap();
        let write = self.database.write().await.unwrap();
        cockpit_authorization::grant(
            &write,
            &session,
            Permission::ChangeState,
            &Scope::Session(session.clone()),
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        Actor::Session {
            session,
            project: Some(project.to_owned()),
        }
    }

    /// The owner adds `title` to `project` as a `kind` item.
    pub async fn item(&self, project: &str, title: &str, kind: Kind) -> Item {
        let write = self.database.write().await.unwrap();
        let settings = cockpit_tasks::Settings {
            blocker_project: None,
            cross_project_open_max: 3,
        };
        let new = NewItem {
            project: Some(project.to_owned()),
            title: title.to_owned(),
            notes: format!("Notes for {title}."),
            kind,
            priority: Priority::NORMAL,
            after: Vec::new(),
        };
        let item = cockpit_tasks::add(&write, &Actor::Owner, &settings, new, NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
        item
    }

    pub async fn start(
        &self,
        item: ItemId,
        preset: &'static Preset,
    ) -> Result<Run, cockpit_workflows::Error> {
        let write = self.database.write().await.unwrap();
        let started = cockpit_workflows::start(&write, &Actor::Owner, item, preset, NOW).await;
        write.commit().await.unwrap();
        started
    }

    /// A `code-change` run on a new item of `project`.
    pub async fn code_change(&self, project: &str) -> Run {
        let item = self.item(project, "Fix the login form", Kind::Bug).await;
        let preset = select_preset(None, item.kind).unwrap();
        self.start(item.id, preset).await.unwrap()
    }

    pub async fn begin(&self, actor: &Actor, run: RunId) -> Result<Run, cockpit_workflows::Error> {
        self.begin_with(actor, run, &observed()).await
    }

    pub async fn begin_with(
        &self,
        actor: &Actor,
        run: RunId,
        capabilities: &Capabilities,
    ) -> Result<Run, cockpit_workflows::Error> {
        let write = self.database.write().await.unwrap();
        let begun = cockpit_workflows::begin(&write, actor, run, capabilities, NOW).await;
        write.commit().await.unwrap();
        begun
    }

    pub async fn checkpoint(
        &self,
        actor: &Actor,
        run: RunId,
        note: &str,
    ) -> Result<Run, cockpit_workflows::Error> {
        let write = self.database.write().await.unwrap();
        let recorded = cockpit_workflows::checkpoint(&write, actor, run, note, NOW + 1).await;
        write.commit().await.unwrap();
        recorded
    }

    pub async fn complete(
        &self,
        actor: &Actor,
        run: RunId,
        completion: Completion,
    ) -> Result<Run, cockpit_workflows::Error> {
        let write = self.database.write().await.unwrap();
        let completed = cockpit_workflows::complete(&write, actor, run, completion, NOW + 2).await;
        write.commit().await.unwrap();
        completed
    }

    pub async fn release(
        &self,
        actor: &Actor,
        run: RunId,
        handoff: Option<Handoff>,
    ) -> Result<Run, cockpit_workflows::Error> {
        let write = self.database.write().await.unwrap();
        let released = cockpit_workflows::release(&write, actor, run, handoff, NOW + 3).await;
        write.commit().await.unwrap();
        released
    }

    pub async fn run(&self, run: RunId) -> Run {
        let read = self.database.read().await.unwrap();
        cockpit_workflows::run(&read, &Actor::Owner, run)
            .await
            .unwrap()
    }

    pub async fn packet(&self, actor: &Actor, run: RunId) -> cockpit_workflows::Packet {
        let read = self.database.read().await.unwrap();
        cockpit_workflows::packet(&read, actor, run).await.unwrap()
    }
}

fn state_path(dir: &Path) -> PathBuf {
    dir.join("state.db")
}

/// What every agent step declares: a provider that reports events and turn outcomes.
pub fn observed() -> Capabilities {
    Capabilities::none()
        .with(
            Capability::Events,
            Provenance::Tested {
                version: "1.0.0".to_owned(),
            },
        )
        .with(Capability::Outcome, Provenance::Discovered)
}

/// A handoff with every field the next session needs.
pub fn handoff(changed: &str) -> Handoff {
    Handoff {
        changed: changed.to_owned(),
        checked: "commits abc123..def456 on greenfield/39-workflows".to_owned(),
        outcome: "cargo nextest: 12 passed, 0 failed".to_owned(),
        needs_human: String::new(),
        evidence: vec!["receipt 7".to_owned()],
    }
}

pub fn completion(changed: &str) -> Completion {
    Completion {
        handoff: handoff(changed),
        satisfied: Vec::new(),
        waived: Vec::new(),
    }
}

/// The refusal inside `result`, failing the test on success or another error.
pub fn refusal<T: std::fmt::Debug>(result: Result<T, cockpit_workflows::Error>) -> Refusal {
    match result {
        Err(cockpit_workflows::Error::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}
