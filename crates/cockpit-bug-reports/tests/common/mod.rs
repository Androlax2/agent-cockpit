//! A scratch state database with the bug reports table.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use cockpit_bug_reports::{Facts, Filed, Kind, NewReport, Source};
use cockpit_turso::Database;

/// 2027-01-15 08:00 UTC: far from a day's edge, so `NOW + an hour` is the same day.
pub const NOW: i64 = 1_800_000_000 - 1_800_000_000 % 86_400 + 8 * 3600;
pub const DAY: i64 = 86_400;

pub struct Fixture {
    pub _dir: tempfile::TempDir,
    pub database: Database,
    pub facts: Facts,
}

impl Fixture {
    pub async fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(
            &dir.path().join("state.db"),
            &[cockpit_bug_reports::MIGRATIONS],
        )
        .await
        .unwrap();
        Fixture {
            _dir: dir,
            database,
            facts: Facts {
                version: "0.1.0".to_owned(),
                environment: "cockpit 0.1.0\nos: linux x86_64".to_owned(),
            },
        }
    }

    pub async fn file_at(
        &self,
        new: NewReport,
        now: i64,
    ) -> Result<Filed, cockpit_bug_reports::Error> {
        let write = self.database.write().await.unwrap();
        let filed = cockpit_bug_reports::file(&write, &self.facts, new, now).await;
        write.commit().await.unwrap();
        filed
    }

    pub async fn file(&self, new: NewReport) -> Result<Filed, cockpit_bug_reports::Error> {
        self.file_at(new, NOW).await
    }
}

pub fn agent(session: &str, description: &str) -> NewReport {
    NewReport {
        source: Source::Agent {
            session: session.to_owned(),
            requested_by_owner: false,
        },
        kind: Kind::Internal,
        component: Some("todo-add".to_owned()),
        title: None,
        description: description.to_owned(),
        steps: String::new(),
    }
}

pub fn requested(session: &str, description: &str) -> NewReport {
    NewReport {
        source: Source::Agent {
            session: session.to_owned(),
            requested_by_owner: true,
        },
        ..agent(session, description)
    }
}

pub fn owner(description: &str) -> NewReport {
    NewReport {
        source: Source::Owner,
        kind: Kind::Other,
        component: None,
        ..agent("unused", description)
    }
}

pub fn daemon(kind: Kind, component: &str) -> NewReport {
    NewReport {
        source: Source::Daemon,
        kind,
        component: Some(component.to_owned()),
        title: None,
        description: format!("the daemon saw a {}", kind.as_str()),
        steps: String::new(),
    }
}
