//! Runs and their steps.
//!
//! The owner starts a run on an open item with a preset; its first step is `ready`. A session
//! `begin`s a ready agent step (its provider must report what the step declares), records
//! `checkpoint`s, and either `complete`s it with a handoff, which makes the next step ready,
//! or `release`s it mid-step with a handoff, which makes it ready again from its checkpoint.
//! The owner completes owner steps, may complete or release any step, waives requirements,
//! and cancels runs. Either may `block` the run on the next action it needs.
//!
//! A session needs `change-state` over itself, granted at launch, and sees only its own
//! project's runs. Nothing here places work: scheduling reads [`ready_steps`].

use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_provider_api::Capabilities;
use cockpit_tasks::ItemId;
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::error::{invalid, invalid_row, session_of};
use crate::presets::preset_version;
use crate::text::{self, BLOCKED_DETAIL_MAX_CHARS, CHECKPOINT_MAX_CHARS};
use crate::{
    Actor, Blocked, BlockedAction, Checkpoint, Doer, Error, Handoff, HandoffKind, HandoffRecord,
    Preset, ReadyStep, Refusal, Run, RunFilter, RunId, RunStatus, Step, StepDefinition, StepStatus,
};

/// The columns `run_from` reads, in its order.
const SELECT_RUNS: &str = "SELECT id, item, project, preset, preset_version, status,
        intent_revision, blocked_action, blocked_detail, blocked_since, created_at, changed_at
     FROM workflows_runs";

/// How a step ends: the handoff, and the evidence its requirements need.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Completion {
    pub handoff: Handoff,
    /// The step's requirements that passed against the exact result, as
    /// `cockpit-verification` decided; the caller passes them, workflows never evaluate.
    pub satisfied: Vec<String>,
    /// The step's requirements the owner waives instead. Only the owner may.
    pub waived: Vec<String>,
}

/// Starts `preset` on an open item; only the owner chooses the workflow a task follows,
/// because the required workflow is part of its intent (report 06).
pub async fn start(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    item: ItemId,
    preset: &'static Preset,
    now: i64,
) -> Result<Run, Error> {
    if *actor != Actor::Owner {
        return Err(Refusal::OwnerOnly {
            action: "start a workflow",
        }
        .into());
    }
    let task = open_item(write, item).await?;
    if let Some(running) = running_run_of(write, item).await? {
        return Err(Refusal::AlreadyRunning { item, run: running }.into());
    }
    let id = write
        .prepare(
            "INSERT INTO workflows_runs (item, project, preset, preset_version, status,
                intent_revision, created_at, changed_at)
             VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6, ?6)
             RETURNING id",
        )
        .await?
        .query_row((
            item.0,
            task.project.as_str(),
            preset.name,
            i64::from(preset.version),
            task.intent_revision,
            now,
        ))
        .await?
        .get::<i64>(0)?;
    for position in preset.positions() {
        let status = if position == 1 {
            StepStatus::Ready
        } else {
            StepStatus::Pending
        };
        write
            .execute(
                "INSERT INTO workflows_steps (run, position, name, status, attempts,
                    checkpoint_revision)
                 VALUES (?1, ?2, ?3, ?4, 0, 0)",
                (
                    id,
                    i64::from(position),
                    step_definition(preset, position)?.name,
                    status.as_str(),
                ),
            )
            .await?;
    }
    required(write, RunId(id)).await
}

pub async fn run(connection: &Connection, actor: &Actor, id: RunId) -> Result<Run, Error> {
    accessible(connection, actor, id).await
}

/// Running runs, oldest first; closed ones too with `include_closed`.
pub async fn runs(
    connection: &Connection,
    actor: &Actor,
    filter: &RunFilter,
) -> Result<Vec<Run>, Error> {
    authorize(connection, actor).await?;
    let project = match actor {
        Actor::Owner => filter.project.as_deref(),
        Actor::Session { project: None, .. } => return Ok(Vec::new()),
        Actor::Session {
            project: Some(own), ..
        } => {
            if filter.project.as_deref().is_some_and(|asked| asked != own) {
                return Ok(Vec::new());
            }
            Some(own.as_str())
        }
    };
    let mut rows = connection
        .query(
            &format!(
                "{SELECT_RUNS} WHERE (?1 IS NULL OR project = ?1)
                    AND (?2 = 1 OR status = 'running')
                 ORDER BY id"
            ),
            (project, i64::from(filter.include_closed)),
        )
        .await?;
    let mut runs = Vec::new();
    while let Some(row) = rows.next().await? {
        runs.push(run_from(&row)?);
    }
    for run in &mut runs {
        run.steps = steps_of(connection, run.id, run.preset).await?;
    }
    Ok(runs)
}

/// Every ready agent step of a running, unblocked run, oldest run first: what scheduling
/// consumes. Owner steps and blocked runs wait on the owner, not on placement.
pub async fn ready_steps(connection: &Connection) -> Result<Vec<ReadyStep>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "{SELECT_RUNS} WHERE status = 'running' AND blocked_action IS NULL
                    AND id IN (SELECT run FROM workflows_steps WHERE status = 'ready')
                 ORDER BY id"
            ),
            (),
        )
        .await?;
    let mut ready = Vec::new();
    while let Some(row) = rows.next().await? {
        ready.push(run_from(&row)?);
    }
    let mut steps = Vec::new();
    for mut run in ready {
        run.steps = steps_of(connection, run.id, run.preset).await?;
        let Some(step) = run.current_step() else {
            continue;
        };
        if step.status == StepStatus::Ready && step.definition.doer == Doer::Agent {
            steps.push(ReadyStep {
                run: run.id,
                item: run.item,
                project: run.project.clone(),
                preset: run.preset,
                position: step.position,
                definition: step.definition,
            });
        }
    }
    Ok(steps)
}

/// Whether a provider reporting `capabilities` may do `step`: dispatch asks before it gives
/// the step to a session, and `begin` asks again. A capability reported as discovered but not
/// tested counts, as it does for a launch (`cockpit-provider-api`).
pub fn check_capabilities(
    step: &'static StepDefinition,
    capabilities: &Capabilities,
) -> Result<(), Refusal> {
    let missing: Vec<_> = step
        .required_capabilities
        .iter()
        .copied()
        .filter(|capability| !capabilities.supports(*capability))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(Refusal::MissingCapabilities {
            step: step.name,
            missing,
        })
    }
}

/// A session takes the run's ready agent step. `capabilities` are those its provider reports.
pub async fn begin(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: RunId,
    capabilities: &Capabilities,
    now: i64,
) -> Result<Run, Error> {
    let Actor::Session { session, .. } = actor else {
        return Err(invalid(
            "actor",
            "a session begins an agent step; the owner completes steps instead",
        )
        .into());
    };
    let run = unblocked_run(write, actor, id).await?;
    open_item(write, run.item).await?;
    let step = current(&run)?;
    if step.definition.doer != Doer::Agent {
        return Err(wrong_step(&run, step, "be begun by a session"));
    }
    if step.status != StepStatus::Ready {
        return Err(wrong_step(&run, step, "be begun"));
    }
    check_capabilities(step.definition, capabilities)?;
    if let Some((other, position)) = step_doing(write, session).await? {
        return Err(Refusal::AlreadyDoing {
            run: other,
            position,
        }
        .into());
    }
    write
        .execute(
            "UPDATE workflows_steps SET status = 'active', session = ?3, attempts = attempts + 1
             WHERE run = ?1 AND position = ?2",
            (id.0, i64::from(step.position), session.as_str()),
        )
        .await?;
    touched(write, id, now).await
}

/// Records the progress the session doing the current step has made; the next session that
/// begins the step resumes from the latest one.
pub async fn checkpoint(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: RunId,
    note: &str,
    now: i64,
) -> Result<Run, Error> {
    if *actor == Actor::Owner {
        return Err(invalid(
            "actor",
            "the session doing the step records its checkpoints",
        )
        .into());
    }
    let note = text::note("note", note, CHECKPOINT_MAX_CHARS)?;
    let run = open_run(write, actor, id).await?;
    let step = current(&run)?;
    session_doing(&run, step, actor)?;
    write
        .execute(
            "UPDATE workflows_steps SET checkpoint = ?3,
                checkpoint_revision = checkpoint_revision + 1, checkpoint_at = ?4
             WHERE run = ?1 AND position = ?2",
            (id.0, i64::from(step.position), note.as_str(), now),
        )
        .await?;
    touched(write, id, now).await
}

/// Ends the current step with its handoff once each requirement passed or was waived; the
/// next step becomes ready, and the run is delivered after the last.
pub async fn complete(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: RunId,
    completion: Completion,
    now: i64,
) -> Result<Run, Error> {
    let handoff = text::handoff(completion.handoff)?;
    let run = unblocked_run(write, actor, id).await?;
    let step = current(&run)?;
    match actor {
        Actor::Owner => {
            if step.status != StepStatus::Ready && step.status != StepStatus::Active {
                return Err(wrong_step(&run, step, "complete"));
            }
        }
        Actor::Session { .. } => {
            if step.definition.doer == Doer::Owner {
                return Err(Refusal::OwnerOnly {
                    action: "complete an owner step",
                }
                .into());
            }
            if !completion.waived.is_empty() {
                return Err(Refusal::OwnerOnly {
                    action: "waive a requirement",
                }
                .into());
            }
            session_doing(&run, step, actor)?;
        }
    }
    let satisfied = requirement_names(step.definition, "satisfied", &completion.satisfied)?;
    let waived = requirement_names(step.definition, "waived", &completion.waived)?;
    let missing: Vec<&'static str> = step
        .definition
        .requirements
        .iter()
        .copied()
        .filter(|requirement| !satisfied.contains(requirement) && !waived.contains(requirement))
        .collect();
    if !missing.is_empty() {
        return Err(Refusal::RequirementsUnmet {
            step: step.definition.name,
            missing,
        }
        .into());
    }
    write
        .execute(
            "UPDATE workflows_steps SET status = 'done', session = NULL, finished_at = ?3
             WHERE run = ?1 AND position = ?2",
            (id.0, i64::from(step.position), now),
        )
        .await?;
    insert_handoff(
        write,
        &run,
        step,
        HandoffKind::Completed,
        actor,
        &handoff,
        &satisfied,
        &waived,
        now,
    )
    .await?;
    let next = step.position + 1;
    if run.preset.step(next).is_some() {
        write
            .execute(
                "UPDATE workflows_steps SET status = 'ready' WHERE run = ?1 AND position = ?2",
                (id.0, i64::from(next)),
            )
            .await?;
    } else {
        write
            .execute(
                "UPDATE workflows_runs SET status = 'delivered' WHERE id = ?1",
                [id.0],
            )
            .await?;
    }
    touched(write, id, now).await
}

/// Hands the active step back mid-step: it is ready again, keeping its checkpoint. The session
/// doing it hands off what the next one needs; the owner, releasing a session that cannot,
/// may hand off nothing.
pub async fn release(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: RunId,
    handoff: Option<Handoff>,
    now: i64,
) -> Result<Run, Error> {
    let handoff = match (actor, handoff) {
        (_, Some(handoff)) => Some(text::handoff(handoff)?),
        (Actor::Owner, None) => None,
        (Actor::Session { .. }, None) => {
            return Err(invalid("handoff", "a session releasing its step hands off").into());
        }
    };
    let run = open_run(write, actor, id).await?;
    let step = current(&run)?;
    if step.status != StepStatus::Active {
        return Err(wrong_step(&run, step, "be released"));
    }
    session_doing(&run, step, actor)?;
    write
        .execute(
            "UPDATE workflows_steps SET status = 'ready', session = NULL
             WHERE run = ?1 AND position = ?2",
            (id.0, i64::from(step.position)),
        )
        .await?;
    if let Some(handoff) = &handoff {
        insert_handoff(
            write,
            &run,
            step,
            HandoffKind::Continued,
            actor,
            handoff,
            &[],
            &[],
            now,
        )
        .await?;
    }
    touched(write, id, now).await
}

/// Marks the run as waiting on `action`; a new block replaces the one before. The session
/// doing the current step or the owner may.
pub async fn block(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: RunId,
    action: BlockedAction,
    detail: &str,
    now: i64,
) -> Result<Run, Error> {
    let detail = text::note("detail", detail, BLOCKED_DETAIL_MAX_CHARS)?;
    let run = open_run(write, actor, id).await?;
    session_doing(&run, current(&run)?, actor)?;
    write
        .execute(
            "UPDATE workflows_runs SET blocked_action = ?2, blocked_detail = ?3,
                blocked_since = ?4
             WHERE id = ?1",
            (id.0, action.as_str(), detail.as_str(), now),
        )
        .await?;
    touched(write, id, now).await
}

pub async fn unblock(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: RunId,
    now: i64,
) -> Result<Run, Error> {
    let run = open_run(write, actor, id).await?;
    session_doing(&run, current(&run)?, actor)?;
    if run.blocked.is_none() {
        return Err(invalid("run", format!("workflow {} is not blocked", id.0)).into());
    }
    write
        .execute(
            "UPDATE workflows_runs SET blocked_action = NULL, blocked_detail = NULL,
                blocked_since = NULL
             WHERE id = ?1",
            [id.0],
        )
        .await?;
    touched(write, id, now).await
}

/// The owner stops the run. Its steps stay as they were, and the item may start another.
pub async fn cancel(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: RunId,
    now: i64,
) -> Result<Run, Error> {
    if *actor != Actor::Owner {
        return Err(Refusal::OwnerOnly {
            action: "cancel a workflow",
        }
        .into());
    }
    open_run(write, actor, id).await?;
    write
        .execute(
            "UPDATE workflows_runs SET status = 'cancelled', blocked_action = NULL,
                blocked_detail = NULL, blocked_since = NULL
             WHERE id = ?1",
            [id.0],
        )
        .await?;
    touched(write, id, now).await
}

/// Every handoff of the run, oldest first.
pub async fn handoffs(
    connection: &Connection,
    actor: &Actor,
    id: RunId,
) -> Result<Vec<HandoffRecord>, Error> {
    let run = accessible(connection, actor, id).await?;
    handoffs_of(connection, &run).await
}

pub(crate) async fn handoffs_of(
    connection: &Connection,
    run: &Run,
) -> Result<Vec<HandoffRecord>, Error> {
    let mut rows = connection
        .query(
            "SELECT position, kind, session, changed, checked, outcome, needs_human, evidence,
                satisfied, waived, created_at
             FROM workflows_handoffs WHERE run = ?1 ORDER BY id",
            [run.id.0],
        )
        .await?;
    let mut records = Vec::new();
    while let Some(row) = rows.next().await? {
        let position = position_of("workflows_handoffs", row.get(0)?)?;
        let kind = row.get::<String>(1)?;
        records.push(HandoffRecord {
            position,
            step: step_definition(run.preset, position)?.name,
            kind: HandoffKind::parse(&kind)
                .ok_or_else(|| invalid_row("workflows_handoffs", format!("kind {kind:?}")))?,
            session: session_of("workflows_handoffs", row.get(2)?)?,
            handoff: Handoff {
                changed: row.get(3)?,
                checked: row.get(4)?,
                outcome: row.get(5)?,
                needs_human: row.get(6)?,
                evidence: lines(&row.get::<String>(7)?),
            },
            satisfied: lines(&row.get::<String>(8)?),
            waived: lines(&row.get::<String>(9)?),
            at: row.get(10)?,
        });
    }
    Ok(records)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one row's columns; a struct for a single private call site adds nothing"
)]
async fn insert_handoff(
    write: &WriteTransaction<'_>,
    run: &Run,
    step: &Step,
    kind: HandoffKind,
    actor: &Actor,
    handoff: &Handoff,
    satisfied: &[&str],
    waived: &[&str],
    now: i64,
) -> Result<(), Error> {
    let session = match actor {
        Actor::Owner => None,
        Actor::Session { session, .. } => Some(session.as_str()),
    };
    write
        .execute(
            "INSERT INTO workflows_handoffs (run, position, kind, session, changed, checked,
                outcome, needs_human, evidence, satisfied, waived, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            (
                run.id.0,
                i64::from(step.position),
                kind.as_str(),
                session,
                handoff.changed.as_str(),
                handoff.checked.as_str(),
                handoff.outcome.as_str(),
                handoff.needs_human.as_str(),
                handoff.evidence.join("\n"),
                satisfied.join("\n"),
                waived.join("\n"),
                now,
            ),
        )
        .await?;
    Ok(())
}

/// The step's requirements named in `names`, refusing a name the step does not declare so a
/// misspelt requirement is never silently ignored.
fn requirement_names(
    step: &'static StepDefinition,
    field: &'static str,
    names: &[String],
) -> Result<Vec<&'static str>, Refusal> {
    let mut found: Vec<&'static str> = Vec::new();
    for name in names {
        let requirement = step
            .requirements
            .iter()
            .copied()
            .find(|requirement| requirement == name)
            .ok_or_else(|| {
                invalid(
                    field,
                    format!("step {} declares no requirement {name:?}", step.name),
                )
            })?;
        if !found.contains(&requirement) {
            found.push(requirement);
        }
    }
    Ok(found)
}

async fn task_item(connection: &Connection, item: ItemId) -> Result<cockpit_tasks::Item, Error> {
    match cockpit_tasks::item(connection, &Actor::Owner, item).await {
        Ok(task) => Ok(task),
        Err(cockpit_tasks::Error::Refused(cockpit_tasks::Refusal::NotFound { .. })) => {
            Err(Refusal::ItemNotFound { item }.into())
        }
        Err(error) => Err(Error::Tasks(error)),
    }
}

pub(crate) async fn task_of(
    connection: &Connection,
    run: &Run,
) -> Result<cockpit_tasks::Item, Error> {
    task_item(connection, run.item).await
}

async fn running_run_of(connection: &Connection, item: ItemId) -> Result<Option<RunId>, Error> {
    let mut rows = connection
        .query(
            "SELECT id FROM workflows_runs WHERE item = ?1 AND status = 'running'",
            [item.0],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(RunId(row.get(0)?))),
        None => Ok(None),
    }
}

/// The step `session` is doing in a running run, if any.
async fn step_doing(
    connection: &Connection,
    session: &SessionId,
) -> Result<Option<(RunId, u32)>, Error> {
    let mut rows = connection
        .query(
            "SELECT s.run, s.position FROM workflows_steps s
                JOIN workflows_runs r ON r.id = s.run
             WHERE s.status = 'active' AND s.session = ?1 AND r.status = 'running'",
            [session.as_str()],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some((
            RunId(row.get(0)?),
            position_of("workflows_steps", row.get(1)?)?,
        ))),
        None => Ok(None),
    }
}

async fn touch(write: &WriteTransaction<'_>, id: RunId, now: i64) -> Result<(), Error> {
    write
        .execute(
            "UPDATE workflows_runs SET changed_at = ?2 WHERE id = ?1",
            (id.0, now),
        )
        .await?;
    Ok(())
}

/// A run the actor may act on, still running.
/// An item a run may still work on: one that is to do or being done.
async fn open_item(connection: &Connection, item: ItemId) -> Result<cockpit_tasks::Item, Error> {
    let task = task_item(connection, item).await?;
    if !matches!(
        task.status,
        cockpit_tasks::Status::Todo | cockpit_tasks::Status::Doing
    ) {
        return Err(Refusal::ItemNotOpen {
            item,
            status: task.status.as_str(),
        }
        .into());
    }
    Ok(task)
}

/// A running run no blocker holds: the only kind whose step may be begun or completed.
async fn unblocked_run(connection: &Connection, actor: &Actor, id: RunId) -> Result<Run, Error> {
    let run = open_run(connection, actor, id).await?;
    if run.blocked.is_some() {
        return Err(Refusal::Blocked { run: id }.into());
    }
    Ok(run)
}

/// Marks the run changed at `now` and reads it back as it now stands.
async fn touched(write: &WriteTransaction<'_>, id: RunId, now: i64) -> Result<Run, Error> {
    touch(write, id, now).await?;
    required(write, id).await
}

async fn open_run(connection: &Connection, actor: &Actor, id: RunId) -> Result<Run, Error> {
    let run = accessible(connection, actor, id).await?;
    if run.status != RunStatus::Running {
        return Err(Refusal::NotRunning {
            run: id,
            status: run.status,
        }
        .into());
    }
    Ok(run)
}

fn current(run: &Run) -> Result<&Step, Error> {
    run.current_step().ok_or_else(|| {
        invalid_row(
            "workflows_steps",
            format!("running workflow {} has no step left", run.id.0),
        )
    })
}

/// Refuses a session that is not doing `step`. The owner passes: it is never "doing" a step.
fn session_doing(run: &Run, step: &Step, actor: &Actor) -> Result<(), Refusal> {
    match actor {
        Actor::Owner => Ok(()),
        Actor::Session { session, .. } => {
            if step.status == StepStatus::Active && step.session.as_ref() == Some(session) {
                Ok(())
            } else {
                Err(Refusal::NotDoing { run: run.id })
            }
        }
    }
}

fn wrong_step(run: &Run, step: &Step, action: &'static str) -> Error {
    Refusal::WrongStep {
        run: run.id,
        position: step.position,
        status: step.status,
        action,
    }
    .into()
}

async fn authorize(connection: &Connection, actor: &Actor) -> Result<(), Error> {
    let Actor::Session { session, .. } = actor else {
        return Ok(());
    };
    let decision = cockpit_authorization::evaluate(
        connection,
        &cockpit_authorization::Actor::Session(session.clone()),
        Permission::ChangeState,
        &Scope::Session(session.clone()),
    )
    .await?;
    match decision.outcome {
        cockpit_authorization::Outcome::Allowed(_) => Ok(()),
        cockpit_authorization::Outcome::Refused(refusal) => {
            Err(Refusal::NotAuthorized(refusal).into())
        }
    }
}

fn is_visible(actor: &Actor, run: &Run) -> bool {
    match actor {
        Actor::Owner => true,
        Actor::Session { project, .. } => project.as_deref() == Some(run.project.as_str()),
    }
}

pub(crate) async fn accessible(
    connection: &Connection,
    actor: &Actor,
    id: RunId,
) -> Result<Run, Error> {
    authorize(connection, actor).await?;
    let run = stored(connection, id)
        .await?
        .filter(|run| is_visible(actor, run))
        .ok_or(Refusal::NotFound { run: id })?;
    Ok(run)
}

/// A run this transaction just changed, so it exists.
async fn required(connection: &Connection, id: RunId) -> Result<Run, Error> {
    stored(connection, id)
        .await?
        .ok_or_else(|| invalid_row("workflows_runs", format!("workflow {} vanished", id.0)))
}

async fn stored(connection: &Connection, id: RunId) -> Result<Option<Run>, Error> {
    let mut rows = connection
        .query(&format!("{SELECT_RUNS} WHERE id = ?1"), [id.0])
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let mut run = run_from(&row)?;
    run.steps = steps_of(connection, run.id, run.preset).await?;
    Ok(Some(run))
}

async fn steps_of(
    connection: &Connection,
    id: RunId,
    preset: &'static Preset,
) -> Result<Vec<Step>, Error> {
    let mut rows = connection
        .query(
            "SELECT position, name, status, session, checkpoint, checkpoint_revision,
                checkpoint_at, attempts, finished_at
             FROM workflows_steps WHERE run = ?1 ORDER BY position",
            [id.0],
        )
        .await?;
    let mut steps = Vec::new();
    while let Some(row) = rows.next().await? {
        steps.push(step_from(&row, preset)?);
    }
    if steps.len() != preset.steps.len() {
        return Err(invalid_row(
            "workflows_steps",
            format!(
                "workflow {} has {} steps; {} version {} has {}",
                id.0,
                steps.len(),
                preset.name,
                preset.version,
                preset.steps.len()
            ),
        ));
    }
    Ok(steps)
}

fn step_from(row: &Row, preset: &'static Preset) -> Result<Step, Error> {
    let position = position_of("workflows_steps", row.get(0)?)?;
    let definition = step_definition(preset, position)?;
    let name = row.get::<String>(1)?;
    if name != definition.name {
        return Err(invalid_row(
            "workflows_steps",
            format!(
                "step {position} is {name:?}; {} version {} names it {:?}",
                preset.name, preset.version, definition.name
            ),
        ));
    }
    let status = row.get::<String>(2)?;
    let checkpoint = match row.get::<Option<String>>(4)? {
        Some(note) => Some(Checkpoint {
            note,
            revision: row.get(5)?,
            at: row.get(6)?,
        }),
        None => None,
    };
    Ok(Step {
        position,
        definition,
        status: StepStatus::parse(&status)
            .ok_or_else(|| invalid_row("workflows_steps", format!("status {status:?}")))?,
        session: session_of("workflows_steps", row.get(3)?)?,
        checkpoint,
        attempts: u32::try_from(row.get::<i64>(7)?)
            .map_err(|error| invalid_row("workflows_steps", format!("attempts: {error}")))?,
        finished_at: row.get(8)?,
    })
}

fn run_from(row: &Row) -> Result<Run, Error> {
    let name = row.get::<String>(3)?;
    let version = row.get::<i64>(4)?;
    let preset = u32::try_from(version)
        .ok()
        .and_then(|version| preset_version(&name, version))
        .ok_or_else(|| {
            invalid_row(
                "workflows_runs",
                format!("no preset {name:?} version {version} in this release"),
            )
        })?;
    let status = row.get::<String>(5)?;
    let blocked = match row.get::<Option<String>>(7)? {
        Some(action) => Some(Blocked {
            action: BlockedAction::parse(&action).ok_or_else(|| {
                invalid_row("workflows_runs", format!("blocked action {action:?}"))
            })?,
            detail: row.get(8)?,
            since: row.get(9)?,
        }),
        None => None,
    };
    Ok(Run {
        id: RunId(row.get(0)?),
        item: ItemId(row.get(1)?),
        project: row.get(2)?,
        preset,
        status: RunStatus::parse(&status)
            .ok_or_else(|| invalid_row("workflows_runs", format!("status {status:?}")))?,
        intent_revision: row.get(6)?,
        blocked,
        steps: Vec::new(),
        created_at: row.get(10)?,
        changed_at: row.get(11)?,
    })
}

fn step_definition(
    preset: &'static Preset,
    position: u32,
) -> Result<&'static StepDefinition, Error> {
    preset.step(position).ok_or_else(|| {
        invalid_row(
            "workflows_steps",
            format!(
                "{} version {} has no step {position}",
                preset.name, preset.version
            ),
        )
    })
}

fn position_of(table: &'static str, position: i64) -> Result<u32, Error> {
    u32::try_from(position).map_err(|error| invalid_row(table, format!("position: {error}")))
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_owned).collect()
}
