//! The task packet: what a session receives for its step (report 06). It is rebuilt from the
//! durable records alone on every read, so a restart, a release or a handoff to another
//! session yields the same packet. Grants are not part of it: they are evaluated at every
//! mediated action, never carried over from a packet.

use cockpit_tasks::{ItemId, Kind, Wait};
use cockpit_turso::sql::Connection;

use crate::runs::{accessible, handoffs_of, task_of};
use crate::{Actor, Blocked, Error, HandoffRecord, NextAction, Preset, RunId, RunStatus, Step};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub run: RunId,
    pub run_status: RunStatus,
    pub preset: &'static Preset,
    pub goal: Goal,
    /// The project the run's sessions work in: the authorized scope of its steps.
    pub project: String,
    /// The current step and who holds it; `None` once the run is delivered or cancelled.
    pub step: Option<Step>,
    pub next_action: NextAction,
    pub blocked: Option<Blocked>,
    /// What the items this one waited on passed on when they finished: the decisions it
    /// builds on.
    pub decisions: Vec<Wait>,
    /// Every handoff of the run so far, oldest first.
    pub handoffs: Vec<HandoffRecord>,
}

/// What the task asks for, as it stands now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Goal {
    pub item: ItemId,
    pub title: String,
    pub notes: String,
    pub kind: Kind,
    pub item_status: cockpit_tasks::Status,
    /// The item's intent revision when the run started.
    pub started_on_intent: i64,
    /// Its intent revision now.
    pub intent_revision: i64,
}

impl Goal {
    /// The goal or scope changed since the run started: approvals and authorization given on
    /// the earlier intent are re-evaluated (report 06, "intent revision").
    pub fn is_intent_changed(&self) -> bool {
        self.intent_revision != self.started_on_intent
    }
}

pub async fn packet(connection: &Connection, actor: &Actor, id: RunId) -> Result<Packet, Error> {
    let run = accessible(connection, actor, id).await?;
    let task = task_of(connection, &run).await?;
    let handoffs = handoffs_of(connection, &run).await?;
    let step = run.current_step().cloned();
    Ok(Packet {
        run: run.id,
        run_status: run.status,
        preset: run.preset,
        goal: Goal {
            item: task.id,
            title: task.title,
            notes: task.notes,
            kind: task.kind,
            item_status: task.status,
            started_on_intent: run.intent_revision,
            intent_revision: task.intent_revision,
        },
        project: run.project.clone(),
        step,
        next_action: run.next_action(),
        blocked: run.blocked.clone(),
        decisions: task
            .waits
            .into_iter()
            .filter(|wait| wait.summary.is_some())
            .collect(),
        handoffs,
    })
}
