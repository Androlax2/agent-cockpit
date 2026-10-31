//! A scratch state database with the tasks and authorization tables.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_tasks::{Actor, Item, ItemId, Kind, NewItem, Priority, Settings};
use cockpit_turso::Database;

pub const NOW: i64 = 1_800_000_000;

pub struct Fixture {
    pub _dir: tempfile::TempDir,
    pub database: Database,
    pub settings: Settings,
}

impl Fixture {
    pub async fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(
            &dir.path().join("state.db"),
            &[cockpit_authorization::MIGRATIONS, cockpit_tasks::MIGRATIONS],
        )
        .await
        .unwrap();
        Fixture {
            _dir: dir,
            database,
            settings: Settings {
                blocker_project: Some("dotfiles".to_owned()),
                cross_project_open_max: 2,
            },
        }
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

    pub async fn add(&self, actor: &Actor, new: NewItem) -> Result<Item, cockpit_tasks::Error> {
        let write = self.database.write().await.unwrap();
        let added = cockpit_tasks::add(&write, actor, &self.settings, new, NOW).await;
        write.commit().await.unwrap();
        added
    }

    /// The owner adds `title` to `project`.
    pub async fn owner_adds(&self, project: &str, title: &str) -> Item {
        self.add(&Actor::Owner, new_item(Some(project), title))
            .await
            .unwrap()
    }

    pub async fn propose(
        &self,
        actor: &Actor,
        project: Option<&str>,
        title: &str,
    ) -> Result<Item, cockpit_tasks::Error> {
        let write = self.database.write().await.unwrap();
        let proposed = cockpit_tasks::propose(
            &write,
            actor,
            &self.settings,
            new_item(project, title),
            "It would save a step every day.",
            NOW,
        )
        .await;
        write.commit().await.unwrap();
        proposed
    }

    pub async fn item(&self, id: ItemId) -> Item {
        let read = self.database.read().await.unwrap();
        cockpit_tasks::item(&read, &Actor::Owner, id).await.unwrap()
    }
}

pub fn new_item(project: Option<&str>, title: &str) -> NewItem {
    NewItem {
        project: project.map(str::to_owned),
        title: title.to_owned(),
        notes: String::new(),
        kind: Kind::Task,
        priority: Priority::NORMAL,
        after: Vec::new(),
    }
}

/// The refusal inside `result`, failing the test on success or another error.
pub fn refusal<T: std::fmt::Debug>(
    result: Result<T, cockpit_tasks::Error>,
) -> cockpit_tasks::Refusal {
    match result {
        Err(cockpit_tasks::Error::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}
