//! A scratch state database with the tables a pass reads and writes, the owner's items, and
//! sessions as the daemon would pass them.
#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_provider_api::{Capabilities, Capability, Provenance};
use cockpit_resource_admission::{Demand, Policy};
use cockpit_scheduling::{
    Candidate, Dispatched, Economy, Explanation, Inputs, Presence, Settings, UsageWindow,
};
use cockpit_tasks::{Actor, Item, ItemId, Kind, NewItem, Priority};
use cockpit_turso::Database;
use cockpit_workflows::{Preset, Run};

pub const NOW: i64 = 1_800_000_000;

/// The account every fixture session runs on.
pub const ACCOUNT: &str = "work";

/// One agent turn's demand in these fixtures.
pub const TURN: Demand = Demand {
    cpu_millis: 1000,
    memory_mib: 1000,
};

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub database: Database,
    pub settings: Settings,
    pub policy: Policy,
}

pub fn migrations() -> [cockpit_turso::MigrationSet; 6] {
    [
        cockpit_authorization::MIGRATIONS,
        cockpit_messaging::MIGRATIONS,
        cockpit_tasks::MIGRATIONS,
        cockpit_resource_admission::MIGRATIONS,
        cockpit_workflows::MIGRATIONS,
        cockpit_scheduling::MIGRATIONS,
    ]
}

impl Fixture {
    /// A budget of `turns` agent turns; no startup headroom unless a test asks for it.
    pub async fn with_budget(turns: u64) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let database = Database::open(&state_path(dir.path()), &migrations())
            .await
            .unwrap();
        Fixture {
            dir,
            database,
            settings: Settings {
                turn_demand: TURN,
                startup_headroom_percent: 0,
                ..Settings::default()
            },
            policy: Policy::for_budget(Demand {
                cpu_millis: TURN.cpu_millis * turns,
                memory_mib: TURN.memory_mib * turns,
            }),
        }
    }

    /// The same database, closed and opened again as a restarted daemon opens it.
    pub async fn reopen(self) -> Fixture {
        let Fixture {
            dir,
            database,
            settings,
            policy,
        } = self;
        drop(database);
        let database = Database::open(&state_path(dir.path()), &migrations())
            .await
            .unwrap();
        Fixture {
            dir,
            database,
            settings,
            policy,
        }
    }

    /// A session of `project` at its prompt for an hour, holding `change-state` over itself
    /// as launched sessions do.
    pub async fn session(&self, id: &str, project: &str) -> Candidate {
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
        idle(id, project)
    }

    /// A queued `todo` item of `project`.
    pub async fn queued(&self, project: &str, title: &str, priority: Priority) -> Item {
        let item = self.item(project, title, priority).await;
        let write = self.database.write().await.unwrap();
        let item = cockpit_tasks::set_queued(&write, &Actor::Owner, item.id, true, NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
        item
    }

    /// An unqueued `todo` item of `project`.
    pub async fn item(&self, project: &str, title: &str, priority: Priority) -> Item {
        let write = self.database.write().await.unwrap();
        let settings = cockpit_tasks::Settings {
            blocker_project: None,
            cross_project_open_max: 3,
        };
        let new = NewItem {
            project: Some(project.to_owned()),
            title: title.to_owned(),
            notes: String::new(),
            kind: Kind::Task,
            priority,
            after: Vec::new(),
        };
        let item = cockpit_tasks::add(&write, &Actor::Owner, &settings, new, NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
        item
    }

    /// One pass at `now` over `candidates`, committed.
    pub async fn dispatch_at(&self, now: i64, candidates: &[Candidate]) -> Dispatched {
        self.dispatch_viewing(now, candidates, None).await
    }

    pub async fn dispatch_viewing(
        &self,
        now: i64,
        candidates: &[Candidate],
        viewed: Option<&str>,
    ) -> Dispatched {
        self.dispatch_with(
            now,
            candidates,
            viewed,
            Vec::new(),
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .await
    }

    /// One pass at `now` over `candidates`, with the accounts' plan windows at `usage`.
    pub async fn dispatch_using(
        &self,
        now: i64,
        candidates: &[Candidate],
        usage: Vec<UsageWindow>,
    ) -> Dispatched {
        self.dispatch_with(
            now,
            candidates,
            None,
            usage,
            BTreeMap::new(),
            BTreeMap::new(),
        )
        .await
    }

    /// One pass at `now` over `candidates`, each item having had its `agent_seconds` (#412).
    pub async fn dispatch_after(
        &self,
        now: i64,
        candidates: &[Candidate],
        agent_seconds: BTreeMap<ItemId, u64>,
    ) -> Dispatched {
        self.dispatch_with(
            now,
            candidates,
            None,
            Vec::new(),
            agent_seconds,
            BTreeMap::new(),
        )
        .await
    }

    /// One pass at `now` over `candidates`, with each project's `economy` (#412).
    pub async fn dispatch_economy(
        &self,
        now: i64,
        candidates: &[Candidate],
        economy: BTreeMap<String, Economy>,
    ) -> Dispatched {
        self.dispatch_with(now, candidates, None, Vec::new(), BTreeMap::new(), economy)
            .await
    }

    /// The owner marks `item` to run economy.
    pub async fn mark_economy(&self, item: ItemId) {
        let write = self.database.write().await.unwrap();
        cockpit_tasks::set_economy(&write, &Actor::Owner, item, true, NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    /// The owner gives `item` a budget of `minutes` agent minutes.
    pub async fn budget(&self, item: ItemId, minutes: u32) {
        let write = self.database.write().await.unwrap();
        cockpit_tasks::set_budget(&write, &Actor::Owner, item, Some(minutes), NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    async fn dispatch_with(
        &self,
        now: i64,
        candidates: &[Candidate],
        viewed: Option<&str>,
        usage: Vec<UsageWindow>,
        agent_seconds: BTreeMap<ItemId, u64>,
        economy: BTreeMap<String, Economy>,
    ) -> Dispatched {
        let write = self.database.write().await.unwrap();
        let inputs = Inputs {
            candidates: candidates.to_vec(),
            viewed: viewed.map(|id| SessionId::new(id).unwrap()),
            usage,
            agent_seconds,
            economy,
            now,
        };
        let dispatched =
            cockpit_scheduling::dispatch(&write, &inputs, &self.settings, &self.policy, &[])
                .await
                .unwrap();
        write.commit().await.unwrap();
        dispatched
    }

    pub async fn explanations(&self) -> Vec<Explanation> {
        let read = self.database.read().await.unwrap();
        cockpit_scheduling::explanations(&read).await.unwrap()
    }

    pub async fn awaits_pass(&self, item: ItemId) -> bool {
        let read = self.database.read().await.unwrap();
        cockpit_scheduling::awaits_pass(&read, item).await.unwrap()
    }

    /// Starts `preset` on `item`, as the owner does.
    pub async fn run(&self, item: ItemId, preset: &'static Preset) -> Run {
        let write = self.database.write().await.unwrap();
        let run = cockpit_workflows::start(&write, &Actor::Owner, item, preset, NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
        run
    }

    /// Pauses admission's queue.
    pub async fn pause_queue(&self) {
        let write = self.database.write().await.unwrap();
        cockpit_resource_admission::set_queue_paused(&write, true, NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
    }

    /// The session that took `item` finishes it.
    pub async fn finish(&self, session: &str, project: &str, item: ItemId) {
        let write = self.database.write().await.unwrap();
        cockpit_tasks::finish(&write, &agent(session, project), item, Some("did it"), NOW)
            .await
            .unwrap();
        write.commit().await.unwrap();
    }
}

fn state_path(dir: &Path) -> PathBuf {
    dir.join("state.db")
}

pub fn agent(session: &str, project: &str) -> Actor {
    Actor::Session {
        session: SessionId::new(session).unwrap(),
        project: Some(project.to_owned()),
    }
}

/// A session at its prompt since an hour before [`NOW`], without a grant, whose provider
/// types messages to it.
pub fn idle(id: &str, project: &str) -> Candidate {
    Candidate {
        session: SessionId::new(id).unwrap(),
        project: Some(project.to_owned()),
        role: None,
        capabilities: observed().with(Capability::Submit, Provenance::Discovered),
        presence: Presence::Waiting { since: NOW - 3600 },
        provider: "claude".to_owned(),
        account: Some(ACCOUNT.to_owned()),
        model: None,
        effort: None,
    }
}

/// What every agent step declares: a provider that reports events and turn outcomes.
pub fn observed() -> Capabilities {
    Capabilities::none()
        .with(Capability::Events, Provenance::Discovered)
        .with(Capability::Outcome, Provenance::Discovered)
}

pub fn minutes(count: u64) -> Duration {
    Duration::from_secs(count * 60)
}

/// The explanation of `item` in the last pass.
pub fn explanation_of(explanations: &[Explanation], item: ItemId) -> &Explanation {
    explanations
        .iter()
        .find(|explanation| explanation.item == item)
        .unwrap_or_else(|| panic!("no explanation of item {}: {explanations:?}", item.0))
}

/// Each placement's project, in placement order.
pub fn placed_projects(dispatched: &Dispatched) -> Vec<&str> {
    dispatched
        .placed
        .iter()
        .map(|placement| placement.project.as_str())
        .collect()
}
