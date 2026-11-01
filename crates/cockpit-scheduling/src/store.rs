//! The pass over the state database: what waits since when, the placements and the
//! explanation records. A pass runs in the caller's write transaction, so what it reads, the
//! work it takes for a session and the records it writes are one change.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_messaging::{Coalescing, DeliveryState, Envelope, Redelivery};
use cockpit_resource_admission::{
    Admission, AdmissionRequest, ControlSupport, Demand, Hold, Limits, Policy, ReservationId,
    Verdict, Workload,
};
use cockpit_tasks::{Item, ItemId, ListFilter, Status};
use cockpit_turso::sql::{Connection, Row};
use cockpit_turso::{Migration, MigrationSet, WriteTransaction};
use cockpit_workflows::{Actor, NextAction, Run, RunFilter, RunId, StepStatus};

use crate::plan::{self, Context, Waiting};
use crate::{
    ActionBy, Dispatched, Error, Explanation, Inputs, Placement, PlacementId, Reason, SessionNote,
    Settings, Unit,
};

pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "scheduling",
    migrations: &[
        Migration {
            version: 1,
            // A unit names its item (and run) without foreign keys: they belong to tasks and
            // workflows, which may delete them; a pass drops what is gone.
            sql: "CREATE TABLE scheduling_waiting (
                unit TEXT PRIMARY KEY CHECK (unit <> ''),
                item INTEGER NOT NULL,
                since INTEGER NOT NULL
            );
            CREATE TABLE scheduling_placements (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                item INTEGER NOT NULL,
                run INTEGER,
                position INTEGER CHECK (position >= 1),
                step TEXT CHECK (step <> ''),
                project TEXT NOT NULL CHECK (project <> ''),
                session TEXT NOT NULL CHECK (session <> ''),
                cpu_millis INTEGER NOT NULL CHECK (cpu_millis >= 0),
                memory_mib INTEGER NOT NULL CHECK (memory_mib >= 0),
                placed_at INTEGER NOT NULL,
                ended_at INTEGER,
                ended_reason TEXT CHECK (ended_reason IN ('work-done', 'work-ended',
                    'session-gone')),
                CHECK ((run IS NULL) = (position IS NULL)),
                CHECK ((run IS NULL) = (step IS NULL)),
                CHECK ((ended_at IS NULL) = (ended_reason IS NULL))
            );
            CREATE UNIQUE INDEX scheduling_placements_one_per_session
                ON scheduling_placements (session) WHERE ended_at IS NULL;
            CREATE UNIQUE INDEX scheduling_placements_one_per_item
                ON scheduling_placements (item) WHERE ended_at IS NULL;
            CREATE INDEX scheduling_placements_finished
                ON scheduling_placements (project) WHERE ended_reason = 'work-done';
            CREATE TABLE scheduling_idle (
                session TEXT PRIMARY KEY CHECK (session <> ''),
                since INTEGER NOT NULL
            );
            CREATE TABLE scheduling_explanations (
                item INTEGER PRIMARY KEY,
                position INTEGER NOT NULL UNIQUE,
                project TEXT NOT NULL CHECK (project <> ''),
                title TEXT NOT NULL CHECK (title <> ''),
                run INTEGER,
                step_position INTEGER CHECK (step_position >= 1),
                step TEXT CHECK (step <> ''),
                reason TEXT NOT NULL CHECK (reason IN ('not-queued', 'waits-on-items',
                    'run-blocked', 'owner-step', 'queue-paused', 'control-unsupported',
                    'infeasible', 'no-room', 'behind', 'no-session', 'no-session-free')),
                why TEXT NOT NULL CHECK (why <> ''),
                limited_by TEXT CHECK (limited_by <> ''),
                sessions TEXT NOT NULL,
                behind INTEGER,
                action TEXT NOT NULL CHECK (action <> ''),
                action_by TEXT NOT NULL CHECK (action_by IN ('owner', 'cockpit')),
                priority INTEGER NOT NULL CHECK (priority BETWEEN 1 AND 6),
                aging_boost INTEGER NOT NULL CHECK (aging_boost >= 0),
                cpu_millis INTEGER NOT NULL CHECK (cpu_millis >= 0),
                memory_mib INTEGER NOT NULL CHECK (memory_mib >= 0),
                headroom_percent INTEGER NOT NULL CHECK (headroom_percent >= 0),
                waiting_since INTEGER,
                evaluated_at INTEGER NOT NULL,
                next_evaluation_at INTEGER NOT NULL,
                CHECK ((run IS NULL) = (step_position IS NULL)),
                CHECK ((run IS NULL) = (step IS NULL)),
                CHECK ((reason = 'behind') = (behind IS NOT NULL))
            );",
        },
        // #385: a placement whose delivery to its agent failed ends `delivery-failed`.
        // #386: it holds an admission reservation, released when it ends; one made before
        // has none and the pass still charges it itself.
        Migration {
            version: 2,
            sql: "CREATE TABLE scheduling_placements_delivered (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                item INTEGER NOT NULL,
                run INTEGER,
                position INTEGER CHECK (position >= 1),
                step TEXT CHECK (step <> ''),
                project TEXT NOT NULL CHECK (project <> ''),
                session TEXT NOT NULL CHECK (session <> ''),
                cpu_millis INTEGER NOT NULL CHECK (cpu_millis >= 0),
                memory_mib INTEGER NOT NULL CHECK (memory_mib >= 0),
                placed_at INTEGER NOT NULL,
                ended_at INTEGER,
                ended_reason TEXT CHECK (ended_reason IN ('work-done', 'work-ended',
                    'session-gone', 'delivery-failed')),
                reservation INTEGER UNIQUE,
                CHECK ((run IS NULL) = (position IS NULL)),
                CHECK ((run IS NULL) = (step IS NULL)),
                CHECK ((ended_at IS NULL) = (ended_reason IS NULL))
            );
            INSERT INTO scheduling_placements_delivered (id, item, run, position, step,
                    project, session, cpu_millis, memory_mib, placed_at, ended_at,
                    ended_reason)
                SELECT id, item, run, position, step, project, session, cpu_millis,
                    memory_mib, placed_at, ended_at, ended_reason
                FROM scheduling_placements;
            DROP TABLE scheduling_placements;
            ALTER TABLE scheduling_placements_delivered RENAME TO scheduling_placements;
            CREATE UNIQUE INDEX scheduling_placements_one_per_session
                ON scheduling_placements (session) WHERE ended_at IS NULL;
            CREATE UNIQUE INDEX scheduling_placements_one_per_item
                ON scheduling_placements (item) WHERE ended_at IS NULL;
            CREATE INDEX scheduling_placements_finished
                ON scheduling_placements (project) WHERE ended_reason = 'work-done';",
        },
    ],
};

/// The producer of a placement's delivery envelope in `cockpit-messaging`'s outbox.
pub const DELIVERY_PRODUCER: &str = "scheduling";

/// Attempts at typing a placement to its agent before it ends `delivery-failed`.
const DELIVERY_ATTEMPTS: u32 = 3;

/// The effect key of `placement`'s delivery: one per placement, so a pass that enqueues it
/// again finds it there.
pub fn delivery_key(placement: PlacementId) -> String {
    format!("placement-{}", placement.0)
}

/// The placement a delivery's effect key names; `None` for a key no placement made.
pub fn placement_of_delivery(effect_key: &str) -> Option<PlacementId> {
    effect_key
        .strip_prefix("placement-")?
        .parse()
        .ok()
        .map(PlacementId)
}

/// One deterministic pass (report 10, "routine dispatch"): ends the placements whose work
/// ended, places what can start now on the sessions that can take it, project by project,
/// and records why everything else waits. The work is taken for the session in `write`
/// (`cockpit-tasks` start for a queued item, `cockpit-workflows` begin for a ready step), so
/// a placement and its work commit together.
///
/// An agent turn is charged against `policy`'s budget beside admission's reservations; a hold
/// admission would give it (paused queue, a control, a demand that never fits) holds it too.
pub async fn dispatch(
    write: &WriteTransaction<'_>,
    inputs: &Inputs,
    settings: &Settings,
    policy: &Policy,
    support: &[ControlSupport],
) -> Result<Dispatched, Error> {
    let now = inputs.now;
    let mut items = cockpit_tasks::list(write, &Actor::Owner, &ListFilter::default()).await?;
    let mut runs = cockpit_workflows::runs(write, &Actor::Owner, &RunFilter::default()).await?;
    let settled = settle(write, &items, &runs, inputs).await?;
    if settled.handed_back {
        items = cockpit_tasks::list(write, &Actor::Owner, &ListFilter::default()).await?;
        runs = cockpit_workflows::runs(write, &Actor::Owner, &RunFilter::default()).await?;
    }
    let ended = settled.ended;
    let candidates = idle_since(write, &inputs.candidates).await?;
    let inputs = &Inputs {
        candidates,
        viewed: inputs.viewed.clone(),
        now,
    };
    let (waiting, outside) = units(&items, &runs);
    let since = record_waiting(write, &waiting, now).await?;
    let waiting: Vec<Waiting> = waiting
        .into_iter()
        .map(|mut unit| {
            unit.since = since[&unit_key(&unit.unit, unit.item)];
            unit
        })
        .collect();
    let active = placements(write).await?;
    let mut placed_by_project: BTreeMap<String, u32> = BTreeMap::new();
    // A placement's admission reservation charges it; only one placed before they did has
    // to be charged here.
    let mut unreserved_demand = Demand::default();
    for placement in &active {
        *placed_by_project
            .entry(placement.project.clone())
            .or_default() += 1;
        if placement.reservation.is_none() {
            unreserved_demand.cpu_millis += placement.demand.cpu_millis;
            unreserved_demand.memory_mib += placement.demand.memory_mib;
        }
    }
    let in_use = cockpit_resource_admission::in_use(write, now).await?;
    let free = plan::minus(plan::minus(policy.budget, in_use.all), unreserved_demand);
    let familiar = familiar_projects(write).await?;
    let turn = settings.turn_demand;
    let unfamiliar = plan::with_headroom(turn, settings.startup_headroom_percent);
    let unavailable = unavailable(write, &items, &runs, inputs).await?;
    let plan = plan::plan(
        waiting,
        Context {
            settings,
            now,
            candidates: &inputs.candidates,
            viewed: inputs.viewed.as_ref(),
            unavailable: &unavailable,
            placed_by_project,
            familiar: &familiar,
            familiar_hold: admission_hold(write, turn, policy, support, now).await?,
            unfamiliar_hold: admission_hold(write, unfamiliar, policy, support, now).await?,
            free,
        },
    );
    let mut placed = Vec::with_capacity(plan.placements.len());
    for planned in plan.placements {
        placed.push(place(write, planned, &inputs.candidates, policy, support, now).await?);
    }
    let mut explanations = plan.explanations;
    explanations.extend(
        outside
            .into_iter()
            .map(|outside| outside.explain(now, settings)),
    );
    replace_explanations(write, &explanations).await?;
    Ok(Dispatched {
        placed,
        ended,
        explanations,
    })
}

/// What admission holds an agent turn of `demand` for, room aside: the pass counts room
/// itself, so one pass places several turns against the same budget.
async fn admission_hold(
    connection: &Connection,
    demand: Demand,
    policy: &Policy,
    support: &[ControlSupport],
    now: i64,
) -> Result<Option<Hold>, Error> {
    let request = AdmissionRequest {
        label: "agent turn".to_owned(),
        workload: Workload::AgentTurn,
        demand,
        limits: Limits::default(),
        required: Vec::new(),
    };
    Ok(
        match cockpit_resource_admission::check(connection, &request, policy, support, now).await? {
            Verdict::Fits | Verdict::Held(Hold::NoRoom { .. }) => None,
            Verdict::Held(hold) => Some(hold),
        },
    )
}

/// An open item that waits outside the queue, on something scheduling does not decide.
struct Outside<'a> {
    item: &'a Item,
    unit: Unit,
    reason: Reason,
    why: String,
    action: String,
}

impl Outside<'_> {
    fn explain(self, now: i64, settings: &Settings) -> Explanation {
        Explanation {
            item: self.item.id,
            project: self.item.project.clone(),
            title: self.item.title.clone(),
            unit: self.unit,
            reason: self.reason,
            why: self.why,
            limited_by: None,
            sessions: Vec::new(),
            behind: None,
            action: self.action,
            action_by: ActionBy::Owner,
            priority: self.item.priority.level(),
            aging_boost: 0,
            demand: Demand::default(),
            headroom_percent: 0,
            waiting_since: None,
            evaluated_at: now,
            next_evaluation_at: now + plan::seconds(settings.interval),
        }
    }
}

/// Splits the open items into queue units and those waiting on something else. An item a
/// running workflow follows waits as its run's ready step; one without a run waits as itself
/// once queued and free of unfinished waits.
fn units<'a>(items: &'a [Item], runs: &[Run]) -> (Vec<Waiting>, Vec<Outside<'a>>) {
    let run_of: BTreeMap<ItemId, &Run> = runs.iter().map(|run| (run.item, run)).collect();
    let (mut waiting, mut outside) = (Vec::new(), Vec::new());
    for item in items {
        if let Some(run) = run_of.get(&item.id) {
            let Some(step) = run.current_step() else {
                continue;
            };
            let unit = Unit::Step {
                run: run.id,
                position: step.position,
                name: step.definition.name,
            };
            match run.next_action() {
                NextAction::AwaitPlacement { .. } => waiting.push(Waiting {
                    item: item.id,
                    project: item.project.clone(),
                    title: item.title.clone(),
                    unit,
                    step: Some(step.definition),
                    role: None,
                    priority: item.priority.level(),
                    since: 0,
                }),
                NextAction::Unblock { action } => outside.push(Outside {
                    item,
                    unit,
                    reason: Reason::RunBlocked,
                    why: format!(
                        "workflow run {} is blocked: {}{}",
                        run.id.0,
                        action.as_str(),
                        run.blocked
                            .as_ref()
                            .map(|blocked| format!(" ({})", blocked.detail))
                            .unwrap_or_default()
                    ),
                    action: format!("unblock it: `cockpit workflow unblock {}`", run.id.0),
                }),
                NextAction::OwnerCompletes { .. } => outside.push(Outside {
                    item,
                    unit,
                    reason: Reason::OwnerStep,
                    why: format!(
                        "step {} of workflow run {} is the owner's",
                        step.definition.name, run.id.0
                    ),
                    action: format!("complete it: `cockpit workflow complete {} ...`", run.id.0),
                }),
                NextAction::InProgress { .. } | NextAction::Nothing => {}
            }
            continue;
        }
        if item.status != Status::Todo {
            continue;
        }
        let waits_on = item.waiting_on();
        if !waits_on.is_empty() {
            let names = item_list(&waits_on);
            outside.push(Outside {
                item,
                unit: Unit::Item,
                reason: Reason::WaitsOnItems,
                why: format!("it waits on {names}, not done yet"),
                action: format!(
                    "finish {names}, or remove the wait: `cockpit todo unwait {} ...`",
                    item.id.0
                ),
            });
        } else if !item.is_queued {
            outside.push(Outside {
                item,
                unit: Unit::Item,
                reason: Reason::NotQueued,
                why: "it is not queued".to_owned(),
                action: format!("queue it: `cockpit todo queue {}`", item.id.0),
            });
        } else {
            waiting.push(Waiting {
                item: item.id,
                project: item.project.clone(),
                title: item.title.clone(),
                unit: Unit::Item,
                step: None,
                role: None,
                priority: item.priority.level(),
                since: 0,
            });
        }
    }
    (waiting, outside)
}

fn item_list(items: &[ItemId]) -> String {
    let numbers: Vec<String> = items.iter().map(|item| item.0.to_string()).collect();
    match numbers.as_slice() {
        [one] => format!("item {one}"),
        _ => format!("items {}", numbers.join(", ")),
    }
}

/// A unit's key: a queued item, or one step of a run. A later step of the same run waits
/// anew.
fn unit_key(unit: &Unit, item: ItemId) -> String {
    match unit {
        Unit::Item => format!("item:{}", item.0),
        Unit::Step { run, position, .. } => format!("step:{}:{position}", run.0),
    }
}

/// Keeps when each unit entered the queue across passes and restarts, and forgets the units
/// no longer in it. Answers each unit's key with its time.
async fn record_waiting(
    write: &WriteTransaction<'_>,
    waiting: &[Waiting],
    now: i64,
) -> Result<BTreeMap<String, i64>, Error> {
    let mut stored = BTreeMap::new();
    let mut rows = write
        .query("SELECT unit, since FROM scheduling_waiting", ())
        .await?;
    while let Some(row) = rows.next().await? {
        stored.insert(row.get::<String>(0)?, row.get::<i64>(1)?);
    }
    drop(rows);
    let mut since = BTreeMap::new();
    for unit in waiting {
        let key = unit_key(&unit.unit, unit.item);
        let entered = match stored.remove(&key) {
            Some(entered) => entered,
            None => {
                write
                    .execute(
                        "INSERT INTO scheduling_waiting (unit, item, since) VALUES (?1, ?2, ?3)",
                        (key.as_str(), unit.item.0, now),
                    )
                    .await?;
                now
            }
        };
        since.insert(key, entered);
    }
    for gone in stored.keys() {
        write
            .execute(
                "DELETE FROM scheduling_waiting WHERE unit = ?1",
                [gone.as_str()],
            )
            .await?;
    }
    Ok(since)
}

/// Sessions that cannot take anything this pass, with why: one that does an item or a step
/// already, or that may not change its own state (no `change-state` over itself).
async fn unavailable(
    connection: &Connection,
    items: &[Item],
    runs: &[Run],
    inputs: &Inputs,
) -> Result<HashMap<SessionId, String>, Error> {
    let mut unavailable = HashMap::new();
    for item in items {
        if item.status == Status::Doing
            && let Some(session) = &item.doing_session
        {
            unavailable.insert(session.clone(), format!("it is doing item {}", item.id.0));
        }
    }
    for run in runs {
        for step in &run.steps {
            if step.status == StepStatus::Active
                && let Some(session) = &step.session
            {
                unavailable.insert(
                    session.clone(),
                    format!(
                        "it is doing step {} of workflow run {}",
                        step.definition.name, run.id.0
                    ),
                );
            }
        }
    }
    for candidate in &inputs.candidates {
        if unavailable.contains_key(&candidate.session) {
            continue;
        }
        let decision = cockpit_authorization::evaluate(
            connection,
            &cockpit_authorization::Actor::Session(candidate.session.clone()),
            Permission::ChangeState,
            &Scope::Session(candidate.session.clone()),
        )
        .await?;
        if let cockpit_authorization::Outcome::Refused(_) = decision.outcome {
            unavailable.insert(
                candidate.session.clone(),
                "it may not change its own state: the owner grants it change-state over itself"
                    .to_owned(),
            );
        }
    }
    Ok(unavailable)
}

struct Settled {
    ended: Vec<PlacementId>,
    /// Work of a gone session went back to the queue, so the items and runs read before
    /// changed.
    handed_back: bool,
}

/// Ends each active placement whose work is no longer done by its session, or whose session
/// is gone from the inputs. Its charge is released with it. Work a gone session still held
/// goes back to the queue: the item to `todo`, the step to ready, as the owner would.
async fn settle(
    write: &WriteTransaction<'_>,
    items: &[Item],
    runs: &[Run],
    inputs: &Inputs,
) -> Result<Settled, Error> {
    let mut settled = Settled {
        ended: Vec::new(),
        handed_back: false,
    };
    for placement in placements(write).await? {
        let is_present = inputs
            .candidates
            .iter()
            .any(|candidate| candidate.session == placement.session);
        let is_working = is_working(&placement, items, runs);
        let reason = match (is_working, is_present) {
            // The daemon ends it as soon as delivery fails; this catches a failure it did
            // not get to (a crash, an error), from the outbox's durable state.
            _ if is_undelivered(write, placement.id).await? => {
                if is_working {
                    hand_back(write, &placement, inputs.now).await?;
                    settled.handed_back = true;
                }
                "delivery-failed"
            }
            (true, true) => continue,
            (true, false) => {
                hand_back(write, &placement, inputs.now).await?;
                settled.handed_back = true;
                "session-gone"
            }
            (false, _) if is_done(write, &placement, runs).await? => "work-done",
            (false, _) => "work-ended",
        };
        end(write, &placement, reason, inputs.now).await?;
        settled.ended.push(placement.id);
    }
    Ok(settled)
}

/// Ends `placement` for `reason` and gives back its admission reservation.
async fn end(
    write: &WriteTransaction<'_>,
    placement: &Placement,
    reason: &str,
    now: i64,
) -> Result<(), Error> {
    write
        .execute(
            "UPDATE scheduling_placements SET ended_at = ?2, ended_reason = ?3
             WHERE id = ?1 AND ended_at IS NULL",
            (placement.id.0, now, reason),
        )
        .await?;
    if let Some(reservation) = placement.reservation {
        cockpit_resource_admission::release_held(write, reservation, now).await?;
    }
    Ok(())
}

/// The placement's delivery to its agent failed for good (#385): it ends `delivery-failed`
/// and its work goes back to the queue, as a vanished session's does, so a later pass places
/// it again. Answers `false`, changing nothing, when the placement already ended.
pub async fn delivery_failed(
    write: &WriteTransaction<'_>,
    placement: PlacementId,
    now: i64,
) -> Result<bool, Error> {
    let Some(placement) = active_placement(write, placement).await? else {
        return Ok(false);
    };
    let items = cockpit_tasks::list(write, &Actor::Owner, &ListFilter::default()).await?;
    let runs = cockpit_workflows::runs(write, &Actor::Owner, &RunFilter::default()).await?;
    if is_working(&placement, &items, &runs) {
        hand_back(write, &placement, now).await?;
    }
    end(write, &placement, "delivery-failed", now).await?;
    Ok(true)
}

/// Whether the placement's delivery failed or expired for good: its agent never got the
/// work. A placement from before delivery has no envelope and is not undelivered.
async fn is_undelivered(connection: &Connection, placement: PlacementId) -> Result<bool, Error> {
    let envelope = cockpit_messaging::envelope_by_key(connection, &delivery_key(placement)).await?;
    Ok(envelope.is_some_and(|envelope| {
        matches!(
            envelope.state,
            DeliveryState::Failed | DeliveryState::Expired
        )
    }))
}

/// Whether the placement's session still does its work.
fn is_working(placement: &Placement, items: &[Item], runs: &[Run]) -> bool {
    match &placement.unit {
        Unit::Item => items.iter().any(|item| {
            item.id == placement.item
                && item.status == Status::Doing
                && item.doing_session.as_ref() == Some(&placement.session)
        }),
        Unit::Step { run, position, .. } => runs.iter().any(|candidate| {
            candidate.id == *run
                && candidate.steps.iter().any(|step| {
                    step.position == *position
                        && step.status == StepStatus::Active
                        && step.session.as_ref() == Some(&placement.session)
                })
        }),
    }
}

/// Whether the placement's work was finished, not dropped: only finished work makes its
/// project familiar.
async fn is_done(
    connection: &Connection,
    placement: &Placement,
    runs: &[Run],
) -> Result<bool, Error> {
    Ok(match &placement.unit {
        Unit::Item => match cockpit_tasks::item(connection, &Actor::Owner, placement.item).await {
            Ok(item) => item.status == Status::Done,
            Err(cockpit_tasks::Error::Refused(cockpit_tasks::Refusal::NotFound { .. })) => false,
            Err(error) => return Err(error.into()),
        },
        Unit::Step { run, position, .. } => runs.iter().any(|candidate| {
            candidate.id == *run
                && candidate
                    .steps
                    .iter()
                    .any(|step| step.position == *position && step.status == StepStatus::Done)
        }),
    })
}

async fn hand_back(
    write: &WriteTransaction<'_>,
    placement: &Placement,
    now: i64,
) -> Result<(), Error> {
    match &placement.unit {
        Unit::Item => {
            // Starting the item took it out of the queue; the session's loss is not the
            // owner's choice to stop it, so it goes back in.
            cockpit_tasks::reopen(write, &Actor::Owner, placement.item, now).await?;
            cockpit_tasks::set_queued(write, &Actor::Owner, placement.item, true, now).await?;
        }
        Unit::Step { run, .. } => {
            cockpit_workflows::release(write, &Actor::Owner, *run, None, now).await?;
        }
    }
    Ok(())
}

/// When each candidate's agent began waiting at its prompt. A report while it waits (a label,
/// what it does) moves the report's time but not the wait's start, so the first time a pass
/// saw it waiting is kept until a pass sees it do anything else.
async fn idle_since(
    write: &WriteTransaction<'_>,
    candidates: &[crate::Candidate],
) -> Result<Vec<crate::Candidate>, Error> {
    let mut stored = HashMap::new();
    let mut rows = write
        .query("SELECT session, since FROM scheduling_idle", ())
        .await?;
    while let Some(row) = rows.next().await? {
        stored.insert(row.get::<String>(0)?, row.get::<i64>(1)?);
    }
    drop(rows);
    let mut adjusted = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let key = candidate.session.as_str();
        let kept = stored.remove(key);
        let mut candidate = candidate.clone();
        match (&mut candidate.presence, kept) {
            (crate::Presence::Waiting { since }, Some(first)) => *since = first,
            (crate::Presence::Waiting { since }, None) => {
                write
                    .execute(
                        "INSERT INTO scheduling_idle (session, since) VALUES (?1, ?2)",
                        (key, *since),
                    )
                    .await?;
            }
            (_, Some(_)) => forget_idle(write, key).await?,
            (_, None) => {}
        }
        adjusted.push(candidate);
    }
    for gone in stored.keys() {
        forget_idle(write, gone).await?;
    }
    Ok(adjusted)
}

async fn forget_idle(write: &WriteTransaction<'_>, session: &str) -> Result<(), Error> {
    write
        .execute("DELETE FROM scheduling_idle WHERE session = ?1", [session])
        .await?;
    Ok(())
}

async fn familiar_projects(connection: &Connection) -> Result<BTreeSet<String>, Error> {
    let mut rows = connection
        .query(
            "SELECT DISTINCT project FROM scheduling_placements WHERE ended_reason = 'work-done'",
            (),
        )
        .await?;
    let mut projects = BTreeSet::new();
    while let Some(row) = rows.next().await? {
        projects.insert(row.get::<String>(0)?);
    }
    Ok(projects)
}

/// Takes the planned work for its session, holds its agent turn's admission reservation,
/// records the placement and enqueues its delivery to the agent: all in `write`, so a
/// placement is never recorded without the intent to deliver it.
async fn place(
    write: &WriteTransaction<'_>,
    planned: plan::Planned,
    candidates: &[crate::Candidate],
    policy: &Policy,
    support: &[ControlSupport],
    now: i64,
) -> Result<Placement, Error> {
    let plan::Planned {
        waiting,
        session,
        demand,
    } = planned;
    let actor = Actor::Session {
        session: session.clone(),
        project: Some(waiting.project.clone()),
    };
    let (run, position, step) = match &waiting.unit {
        Unit::Item => {
            cockpit_tasks::start(write, &actor, waiting.item, now).await?;
            (None, None, None)
        }
        Unit::Step {
            run,
            position,
            name,
        } => {
            let capabilities = candidates
                .iter()
                .find(|candidate| candidate.session == session)
                .map(|candidate| candidate.capabilities.clone())
                .unwrap_or_default();
            cockpit_workflows::begin(write, &actor, *run, &capabilities, now).await?;
            (Some(run.0), Some(i64::from(*position)), Some(*name))
        }
    };
    let request = AdmissionRequest {
        label: format!(
            "agent turn: item {} on {}",
            waiting.item.0,
            session.as_str()
        ),
        workload: Workload::AgentTurn,
        demand,
        limits: Limits::default(),
        required: Vec::new(),
    };
    let reservation =
        match cockpit_resource_admission::hold(write, &request, policy, support, now).await? {
            Admission::Admitted(reservation) => reservation,
            // The plan counted room from the same reservations in the same transaction.
            Admission::Held(hold) => {
                return Err(Error::PlanAdmissionMismatch {
                    item: waiting.item,
                    hold: hold.to_string(),
                });
            }
        };
    let row = write
        .prepare(
            "INSERT INTO scheduling_placements (item, run, position, step, project, session,
                 cpu_millis, memory_mib, placed_at, reservation)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             RETURNING id",
        )
        .await?
        .query_row(cockpit_turso::sql::params![
            waiting.item.0,
            run,
            position,
            step,
            waiting.project.as_str(),
            session.as_str(),
            stored(demand.cpu_millis),
            stored(demand.memory_mib),
            now,
            reservation.0,
        ])
        .await?;
    let placement = Placement {
        id: PlacementId(row.get(0)?),
        item: waiting.item,
        unit: waiting.unit,
        project: waiting.project,
        session,
        demand,
        placed_at: now,
        reservation: Some(reservation),
    };
    cockpit_messaging::enqueue(
        write,
        &Envelope {
            effect_key: delivery_key(placement.id),
            producer: DELIVERY_PRODUCER.to_owned(),
            destination: placement.session.as_str().to_owned(),
            payload: placement.brief(),
            // Typing the work twice would start it twice.
            redelivery: Redelivery::Never,
            coalescing: Coalescing::Never,
            max_attempts: DELIVERY_ATTEMPTS,
            expires_at: None,
        },
        now,
    )
    .await?;
    Ok(placement)
}

/// The placement `id` while it is active.
pub async fn active_placement(
    connection: &Connection,
    id: PlacementId,
) -> Result<Option<Placement>, Error> {
    let mut rows = connection
        .query(
            &format!("SELECT {PLACEMENT_COLUMNS} FROM scheduling_placements WHERE id = ?1 AND ended_at IS NULL"),
            [id.0],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(placement_from(&row)?)),
        None => Ok(None),
    }
}

const PLACEMENT_COLUMNS: &str = "id, item, run, position, step, project, session, cpu_millis,
    memory_mib, placed_at, reservation";

/// Every active placement, oldest first.
pub async fn placements(connection: &Connection) -> Result<Vec<Placement>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {PLACEMENT_COLUMNS} FROM scheduling_placements WHERE ended_at IS NULL
                 ORDER BY id"
            ),
            (),
        )
        .await?;
    let mut placements = Vec::new();
    while let Some(row) = rows.next().await? {
        placements.push(placement_from(&row)?);
    }
    Ok(placements)
}

fn placement_from(row: &Row) -> Result<Placement, Error> {
    let invalid = |reason: String| Error::InvalidStoredRow {
        table: "scheduling_placements",
        reason,
    };
    let session: String = row.get(6)?;
    Ok(Placement {
        id: PlacementId(row.get(0)?),
        item: ItemId(row.get(1)?),
        unit: unit_from(row.get(2)?, row.get(3)?, row.get(4)?).map_err(invalid)?,
        project: row.get(5)?,
        session: SessionId::new(session).map_err(|error| invalid(error.to_string()))?,
        demand: Demand {
            cpu_millis: loaded(row.get(7)?).map_err(invalid)?,
            memory_mib: loaded(row.get(8)?).map_err(invalid)?,
        },
        placed_at: row.get(9)?,
        reservation: row.get::<Option<i64>>(10)?.map(ReservationId),
    })
}

/// A stored step names its preset step by name: the name is kept as the static one of the
/// preset whose step it is, so a release that no longer has it fails loud.
fn unit_from(
    run: Option<i64>,
    position: Option<i64>,
    step: Option<String>,
) -> Result<Unit, String> {
    match (run, position, step) {
        (None, None, None) => Ok(Unit::Item),
        (Some(run), Some(position), Some(step)) => Ok(Unit::Step {
            run: RunId(run),
            position: u32::try_from(position).map_err(|_| format!("position {position}"))?,
            name: step_name(&step).ok_or_else(|| format!("step {step:?} is in no preset"))?,
        }),
        (run, position, step) => Err(format!(
            "run {run:?}, position {position:?} and step {step:?} do not name one unit"
        )),
    }
}

fn step_name(name: &str) -> Option<&'static str> {
    cockpit_workflows::PRESETS
        .iter()
        .flat_map(|preset| preset.steps.iter())
        .map(|step| step.name)
        .find(|known| *known == name)
}

async fn replace_explanations(
    write: &WriteTransaction<'_>,
    explanations: &[Explanation],
) -> Result<(), Error> {
    write
        .execute("DELETE FROM scheduling_explanations", ())
        .await?;
    for (position, explanation) in explanations.iter().enumerate() {
        let (run, step_position, step) = match &explanation.unit {
            Unit::Item => (None, None, None),
            Unit::Step {
                run,
                position,
                name,
            } => (Some(run.0), Some(i64::from(*position)), Some(*name)),
        };
        let sessions: Vec<String> = explanation
            .sessions
            .iter()
            .map(|note| format!("{}\t{}", note.session.as_str(), one_line(&note.why)))
            .collect();
        write
            .execute(
                "INSERT INTO scheduling_explanations (item, position, project, title, run,
                     step_position, step, reason, why, limited_by, sessions, behind, action,
                     action_by, priority, aging_boost, cpu_millis, memory_mib, headroom_percent,
                     waiting_since, evaluated_at, next_evaluation_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                     ?16, ?17, ?18, ?19, ?20, ?21, ?22)",
                cockpit_turso::sql::params![
                    explanation.item.0,
                    i64::try_from(position).unwrap_or(i64::MAX),
                    explanation.project.as_str(),
                    explanation.title.as_str(),
                    run,
                    step_position,
                    step,
                    explanation.reason.as_str(),
                    explanation.why.as_str(),
                    explanation.limited_by.as_deref(),
                    sessions.join("\n"),
                    explanation.behind.map(|item| item.0),
                    explanation.action.as_str(),
                    explanation.action_by.as_str(),
                    explanation.priority,
                    explanation.aging_boost,
                    stored(explanation.demand.cpu_millis),
                    stored(explanation.demand.memory_mib),
                    stored(explanation.headroom_percent),
                    explanation.waiting_since,
                    explanation.evaluated_at,
                    explanation.next_evaluation_at,
                ],
            )
            .await?;
    }
    Ok(())
}

/// The explanation records the last pass wrote, in dispatch order: the queue first, then the
/// items waiting on something outside it.
pub async fn explanations(connection: &Connection) -> Result<Vec<Explanation>, Error> {
    let mut rows = connection
        .query(
            "SELECT item, project, title, run, step_position, step, reason, why, limited_by,
                 sessions, behind, action, action_by, priority, aging_boost, cpu_millis,
                 memory_mib, headroom_percent, waiting_since, evaluated_at, next_evaluation_at
             FROM scheduling_explanations ORDER BY position",
            (),
        )
        .await?;
    let mut explanations = Vec::new();
    while let Some(row) = rows.next().await? {
        explanations.push(explanation_from(&row)?);
    }
    Ok(explanations)
}

fn explanation_from(row: &Row) -> Result<Explanation, Error> {
    let invalid = |reason: String| Error::InvalidStoredRow {
        table: "scheduling_explanations",
        reason,
    };
    let reason: String = row.get(6)?;
    let action_by: String = row.get(12)?;
    let sessions: String = row.get(9)?;
    let sessions = sessions
        .lines()
        .map(|line| {
            let (session, why) = line
                .split_once('\t')
                .ok_or_else(|| invalid(format!("session note {line:?}")))?;
            Ok(SessionNote {
                session: SessionId::new(session).map_err(|error| invalid(error.to_string()))?,
                why: why.to_owned(),
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    Ok(Explanation {
        item: ItemId(row.get(0)?),
        project: row.get(1)?,
        title: row.get(2)?,
        unit: unit_from(row.get(3)?, row.get(4)?, row.get(5)?).map_err(invalid)?,
        reason: Reason::parse(&reason).ok_or_else(|| invalid(format!("reason {reason:?}")))?,
        why: row.get(7)?,
        limited_by: row.get(8)?,
        sessions,
        behind: row.get::<Option<i64>>(10)?.map(ItemId),
        action: row.get(11)?,
        action_by: ActionBy::parse(&action_by)
            .ok_or_else(|| invalid(format!("action by {action_by:?}")))?,
        priority: row.get(13)?,
        aging_boost: row.get(14)?,
        demand: Demand {
            cpu_millis: loaded(row.get(15)?).map_err(invalid)?,
            memory_mib: loaded(row.get(16)?).map_err(invalid)?,
        },
        headroom_percent: loaded(row.get(17)?).map_err(invalid)?,
        waiting_since: row.get(18)?,
        evaluated_at: row.get(19)?,
        next_evaluation_at: row.get(20)?,
    })
}

/// A note is stored as one `session\twhy` line.
fn one_line(text: &str) -> String {
    text.replace(['\t', '\n', '\r'], " ")
}

fn stored(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn loaded(value: i64) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("negative value {value}"))
}
