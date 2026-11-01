//! Routes `cockpit workflow` to `cockpit-workflows` and converts its types to the wire at this
//! boundary (#39). The owner acts on the host-only socket; an agent on the agent socket, where
//! `cockpit-agent-channel` identifies the sender and its working directory names its project.
//! The daemon reads the project's `workflow.preset` and the sender's provider capabilities;
//! the rules live in `cockpit-workflows`.
//!
//! No request can claim a requirement passed: until `cockpit-verification` (#40) reports
//! which ones did, a step with requirements completes only with the owner's waiver.

use std::collections::BTreeMap;
use std::sync::Arc;

use cockpit_protocol::{
    Command, ErrorCode, Outcome, ProtocolError, Reply, TaskWaitView, UnblockedBy,
    WorkflowBlockedActionView, WorkflowBlockedView, WorkflowCheckpointView, WorkflowGoalView,
    WorkflowHandoffRecordView, WorkflowHandoffView, WorkflowNextView, WorkflowPacketView,
    WorkflowPresetStepView, WorkflowPresetView, WorkflowStatusView, WorkflowStepStatusView,
    WorkflowStepView, WorkflowView,
};
use cockpit_provider_api::Capabilities;
use cockpit_turso::sql::Connection;
use cockpit_turso::{Database, WriteTransaction};
use cockpit_workflows::{
    Actor, Blocked, BlockedAction, Completion, Handoff, HandoffRecord, NextAction, Packet, Preset,
    Refusal, Run, RunFilter, RunId, RunStatus, Step, StepDefinition, StepStatus,
};

use crate::approvals::{Answer, Failed, Sender, job, now};
use crate::routes::{Peer, Route, WorkClass};
use crate::tasks::{kind_view, refused, status_view as task_status_view, unblocked_by};

/// What a workflow command needs besides the database: the owner's environment for the
/// project's `workflow.preset`, the sessions for the sender's provider, and what each
/// provider reports it can do.
#[derive(Clone)]
pub(crate) struct Context {
    pub(crate) database: Arc<Database>,
    pub(crate) environment: cockpit_config::Environment,
    pub(crate) sessions: Arc<cockpit_sessions::Sessions>,
    pub(crate) provider_capabilities: Arc<BTreeMap<String, Capabilities>>,
}

/// The route for a workflow command; `routes.rs` sends only those here, after the socket
/// rules. Agent requests run in the bulk lane like every agent request (#32).
pub(crate) fn route(command: Command, peer: Peer, context: Context) -> Route {
    if matches!(command, Command::WorkflowPresets) {
        return Route::Answer(Outcome::Ok(presets_reply()));
    }
    let class = match peer {
        Peer::Agent { .. } => WorkClass::Bulk,
        Peer::Owner => WorkClass::Control,
    };
    let work = async move {
        match peer {
            Peer::Owner => owner(command, &context).await,
            Peer::Agent { connector } => {
                let sender = Sender::once_launched(
                    connector,
                    &context.database,
                    launch_token(&command).as_deref(),
                )
                .await?;
                agent(command, sender, &context).await
            }
        }
    };
    Route::Run {
        class,
        job: job(work),
    }
}

async fn owner(command: Command, context: &Context) -> Answer {
    match command {
        Command::WorkflowStart { item, preset } => start(context, item, preset).await,
        command if is_read(&command) => {
            let read = context.database.read().await?;
            read_reply(&read, &Actor::Owner, command).await
        }
        command => {
            let write = context.database.write().await?;
            let reply = dispatch(&write, &Actor::Owner, &Capabilities::none(), command).await?;
            write.commit().await?;
            Ok(reply)
        }
    }
}

/// Identifies the sender and resolves its project (and, to begin a step, its provider) on a
/// read transaction, answers reads there, and takes the writer only to change state. A
/// launch not bound yet is identified, and so bound, under the writer.
async fn agent(command: Command, sender: Sender, context: &Context) -> Answer {
    let token = launch_token(&command);
    let read = context.database.read().await?;
    let bound = sender.identify_bound(&read, token.as_deref()).await?;
    let project = sender.project(&read).await?.map(|project| project.name);
    if let Some(session) = &bound
        && is_read(&command)
    {
        let actor = Actor::Session {
            session: session.clone(),
            project,
        };
        return read_reply(&read, &actor, command).await;
    }
    drop(read);
    let is_begin = matches!(command, Command::WorkflowBegin { .. });
    let mut capabilities = match &bound {
        Some(session) if is_begin => Some(provider_capabilities(context, session).await?),
        _ => None,
    };
    let write = context.database.write().await?;
    let session = sender.identify(&write, token).await?;
    if is_begin && capabilities.is_none() {
        capabilities = Some(provider_capabilities(context, &session).await?);
    }
    let actor = Actor::Session { session, project };
    let reply = if is_read(&command) {
        read_reply(&write, &actor, command).await?
    } else {
        let capabilities = capabilities.unwrap_or_else(Capabilities::none);
        dispatch(&write, &actor, &capabilities, command).await?
    };
    write.commit().await?;
    Ok(reply)
}

/// What the sender's provider reports, from the session the agent channel identified.
async fn provider_capabilities(
    context: &Context,
    session: &cockpit_authorization::SessionId,
) -> Result<Capabilities, Failed> {
    let id = cockpit_sessions::SessionId::parse(session.as_str())?;
    let provider = context.sessions.get(&id).await?.provider;
    context
        .provider_capabilities
        .get(provider.as_str())
        .cloned()
        .ok_or_else(|| {
            refused(
                ErrorCode::Refused,
                format!("this daemon no longer offers the session's provider {provider}"),
                "restart the session with a provider `cockpit session new` offers",
                UnblockedBy::Owner,
            )
        })
}

/// Starts a workflow, choosing the preset the owner named, else the project's
/// `workflow.preset`, else the item kind's default. The configuration is read before the
/// write lock is taken; a broken project configuration refuses the start instead of applying
/// a preset nobody chose.
async fn start(context: &Context, item: i64, asked: Option<String>) -> Answer {
    let (kind, configured) = {
        let read = context.database.read().await?;
        let task = cockpit_tasks::item(&read, &Actor::Owner, cockpit_tasks::ItemId(item))
            .await
            .map_err(|error| match error {
                cockpit_tasks::Error::Refused(cockpit_tasks::Refusal::NotFound { .. }) => {
                    Failed::from(Refusal::ItemNotFound {
                        item: cockpit_tasks::ItemId(item),
                    })
                }
                error => Failed::from(error),
            })?;
        let configured = match asked {
            Some(name) => Some(name),
            None => project_preset(&read, &context.environment, &task.project).await?,
        };
        (task.kind, configured)
    };
    let preset = cockpit_workflows::select_preset(configured.as_deref(), kind)?;
    let write = context.database.write().await?;
    let run = cockpit_workflows::start(
        &write,
        &Actor::Owner,
        cockpit_tasks::ItemId(item),
        preset,
        now(),
    )
    .await?;
    write.commit().await?;
    Ok(Reply::Workflow {
        workflow: Box::new(workflow_view(&run)),
    })
}

/// The project's `workflow.preset`, from its configuration and the owner's.
async fn project_preset(
    connection: &Connection,
    environment: &cockpit_config::Environment,
    project: &str,
) -> Result<Option<String>, Failed> {
    let root = match cockpit_projects::project_named(connection, project).await {
        Ok(project) => project.root,
        Err(cockpit_projects::Error::UnknownProject { .. }) => {
            return Err(refused(
                ErrorCode::NotFound,
                format!("the item's project {project} is not registered"),
                "register it with `cockpit project add`, or name a preset",
                UnblockedBy::Owner,
            ));
        }
        Err(error) => return Err(internal(&error)),
    };
    let environment = environment.clone();
    let loaded =
        tokio::task::spawn_blocking(move || cockpit_config::load(&environment, Some(&root), &[]))
            .await
            .map_err(|error| internal(&error))?;
    let config = loaded.map_err(|error| {
        refused(
            ErrorCode::Refused,
            format!("the project configuration cannot be read: {error}"),
            "fix the configuration file, or name a preset",
            UnblockedBy::Owner,
        )
    })?;
    Ok(config.workflow_preset.value)
}

async fn dispatch(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    capabilities: &Capabilities,
    command: Command,
) -> Answer {
    let now = now();
    let run = match command {
        Command::WorkflowBegin { workflow, .. } => {
            cockpit_workflows::begin(write, actor, RunId(workflow), capabilities, now).await?
        }
        Command::WorkflowCheckpoint { workflow, note, .. } => {
            cockpit_workflows::checkpoint(write, actor, RunId(workflow), &note, now).await?
        }
        Command::WorkflowComplete {
            workflow,
            handoff,
            waive,
            ..
        } => {
            let completion = Completion {
                handoff: handoff_of(handoff),
                satisfied: Vec::new(),
                waived: waive,
            };
            cockpit_workflows::complete(write, actor, RunId(workflow), completion, now).await?
        }
        Command::WorkflowRelease {
            workflow, handoff, ..
        } => {
            cockpit_workflows::release(write, actor, RunId(workflow), handoff.map(handoff_of), now)
                .await?
        }
        Command::WorkflowBlock {
            workflow,
            action,
            detail,
            ..
        } => {
            let action = blocked_action(action)?;
            cockpit_workflows::block(write, actor, RunId(workflow), action, &detail, now).await?
        }
        Command::WorkflowUnblock { workflow, .. } => {
            cockpit_workflows::unblock(write, actor, RunId(workflow), now).await?
        }
        Command::WorkflowCancel { workflow } => {
            cockpit_workflows::cancel(write, actor, RunId(workflow), now).await?
        }
        other => unreachable!("routes.rs sends only workflow commands here, not {other:?}"),
    };
    Ok(Reply::Workflow {
        workflow: Box::new(workflow_view(&run)),
    })
}

fn is_read(command: &Command) -> bool {
    matches!(
        command,
        Command::WorkflowList { .. }
            | Command::WorkflowShow { .. }
            | Command::WorkflowPacket { .. }
    )
}

async fn read_reply(connection: &Connection, actor: &Actor, command: Command) -> Answer {
    Ok(match command {
        Command::WorkflowList { project, all, .. } => {
            let filter = RunFilter {
                project,
                include_closed: all,
            };
            let runs = cockpit_workflows::runs(connection, actor, &filter).await?;
            Reply::Workflows {
                workflows: runs.iter().map(workflow_view).collect(),
            }
        }
        Command::WorkflowShow { workflow, .. } => Reply::Workflow {
            workflow: Box::new(workflow_view(
                &cockpit_workflows::run(connection, actor, RunId(workflow)).await?,
            )),
        },
        Command::WorkflowPacket { workflow, .. } => Reply::WorkflowPacket {
            packet: Box::new(packet_view(
                cockpit_workflows::packet(connection, actor, RunId(workflow)).await?,
            )),
        },
        other => unreachable!("is_read admits only reads, not {other:?}"),
    })
}

/// The launch token a command from a new launch carries.
fn launch_token(command: &Command) -> Option<String> {
    match command {
        Command::WorkflowList { launch_token, .. }
        | Command::WorkflowShow { launch_token, .. }
        | Command::WorkflowPacket { launch_token, .. }
        | Command::WorkflowBegin { launch_token, .. }
        | Command::WorkflowCheckpoint { launch_token, .. }
        | Command::WorkflowComplete { launch_token, .. }
        | Command::WorkflowRelease { launch_token, .. }
        | Command::WorkflowBlock { launch_token, .. }
        | Command::WorkflowUnblock { launch_token, .. } => launch_token.clone(),
        _ => None,
    }
}

fn presets_reply() -> Reply {
    Reply::WorkflowPresets {
        presets: cockpit_workflows::presets()
            .into_iter()
            .map(preset_view)
            .collect(),
    }
}

fn preset_view(preset: &Preset) -> WorkflowPresetView {
    WorkflowPresetView {
        name: preset.name.to_owned(),
        version: preset.version,
        summary: preset.summary.to_owned(),
        steps: preset
            .steps
            .iter()
            .map(|step| WorkflowPresetStepView {
                name: step.name.to_owned(),
                purpose: step.purpose.to_owned(),
                doer: step.doer.as_str().to_owned(),
                requirements: requirements(step),
                required_capabilities: required_capabilities(step),
            })
            .collect(),
    }
}

fn handoff_of(view: WorkflowHandoffView) -> Handoff {
    Handoff {
        changed: view.changed,
        checked: view.checked,
        outcome: view.outcome,
        needs_human: view.needs_human,
        evidence: view.evidence,
    }
}

fn handoff_view(handoff: &Handoff) -> WorkflowHandoffView {
    WorkflowHandoffView {
        changed: handoff.changed.clone(),
        checked: handoff.checked.clone(),
        outcome: handoff.outcome.clone(),
        needs_human: handoff.needs_human.clone(),
        evidence: handoff.evidence.clone(),
    }
}

fn blocked_action(view: WorkflowBlockedActionView) -> Result<BlockedAction, Failed> {
    Ok(match view {
        WorkflowBlockedActionView::AnswerQuestion => BlockedAction::AnswerQuestion,
        WorkflowBlockedActionView::ApproveOperation => BlockedAction::ApproveOperation,
        WorkflowBlockedActionView::RestoreCapability => BlockedAction::RestoreCapability,
        WorkflowBlockedActionView::WaitForCapacity => BlockedAction::WaitForCapacity,
        WorkflowBlockedActionView::ReconcileResult => BlockedAction::ReconcileResult,
        WorkflowBlockedActionView::Unknown => {
            return Err(refused(
                ErrorCode::InvalidArgument,
                "action: unknown blocker action".to_owned(),
                &format!("use one of {}", WorkflowBlockedActionView::known_names()),
                UnblockedBy::Agent,
            ));
        }
    })
}

fn blocked_action_view(action: BlockedAction) -> WorkflowBlockedActionView {
    match action {
        BlockedAction::AnswerQuestion => WorkflowBlockedActionView::AnswerQuestion,
        BlockedAction::ApproveOperation => WorkflowBlockedActionView::ApproveOperation,
        BlockedAction::RestoreCapability => WorkflowBlockedActionView::RestoreCapability,
        BlockedAction::WaitForCapacity => WorkflowBlockedActionView::WaitForCapacity,
        BlockedAction::ReconcileResult => WorkflowBlockedActionView::ReconcileResult,
    }
}

fn workflow_view(run: &Run) -> WorkflowView {
    WorkflowView {
        id: run.id.0,
        item: run.item.0,
        project: run.project.clone(),
        preset: run.preset.name.to_owned(),
        preset_version: run.preset.version,
        status: status_view(run.status),
        intent_revision: run.intent_revision,
        blocked: run.blocked.as_ref().map(blocked_view),
        steps: run.steps.iter().map(step_view).collect(),
        next: next_view(run.next_action()),
        created_at: run.created_at,
        changed_at: run.changed_at,
    }
}

fn packet_view(packet: Packet) -> WorkflowPacketView {
    WorkflowPacketView {
        workflow: packet.run.0,
        status: status_view(packet.run_status),
        preset: packet.preset.name.to_owned(),
        preset_version: packet.preset.version,
        item: packet.item.0,
        goal: packet.goal.map(|goal| WorkflowGoalView {
            intent_changed: goal.is_intent_changed(),
            title: goal.title,
            notes: goal.notes,
            kind: kind_view(goal.kind),
            item_status: task_status_view(goal.item_status),
            started_on_intent: goal.started_on_intent,
            intent_revision: goal.intent_revision,
        }),
        project: packet.project,
        step: packet.step.as_ref().map(step_view),
        next: next_view(packet.next_action),
        blocked: packet.blocked.as_ref().map(blocked_view),
        decisions: packet
            .decisions
            .into_iter()
            .map(|wait| TaskWaitView {
                item: wait.item.0,
                title: wait.title,
                status: task_status_view(wait.status),
                summary: wait.summary,
            })
            .collect(),
        handoffs: packet.handoffs.iter().map(handoff_record_view).collect(),
    }
}

fn handoff_record_view(record: &HandoffRecord) -> WorkflowHandoffRecordView {
    WorkflowHandoffRecordView {
        position: record.position,
        step: record.step.to_owned(),
        kind: record.kind.as_str().to_owned(),
        session: record
            .session
            .as_ref()
            .map(|session| session.as_str().to_owned()),
        handoff: handoff_view(&record.handoff),
        satisfied: record.satisfied.clone(),
        waived: record.waived.clone(),
        at: record.at,
    }
}

fn step_view(step: &Step) -> WorkflowStepView {
    WorkflowStepView {
        position: step.position,
        name: step.definition.name.to_owned(),
        purpose: step.definition.purpose.to_owned(),
        doer: step.definition.doer.as_str().to_owned(),
        status: step_status_view(step.status),
        session: step
            .session
            .as_ref()
            .map(|session| session.as_str().to_owned()),
        checkpoint: step
            .checkpoint
            .as_ref()
            .map(|checkpoint| WorkflowCheckpointView {
                note: checkpoint.note.clone(),
                revision: checkpoint.revision,
                at: checkpoint.at,
            }),
        attempts: step.attempts,
        requirements: requirements(step.definition),
        required_capabilities: required_capabilities(step.definition),
    }
}

fn requirements(step: &StepDefinition) -> Vec<String> {
    step.requirements
        .iter()
        .map(|name| (*name).to_owned())
        .collect()
}

fn required_capabilities(step: &StepDefinition) -> Vec<String> {
    step.required_capabilities
        .iter()
        .map(|capability| capability.as_str().to_owned())
        .collect()
}

fn blocked_view(blocked: &Blocked) -> WorkflowBlockedView {
    WorkflowBlockedView {
        action: blocked_action_view(blocked.action),
        detail: blocked.detail.clone(),
        since: blocked.since,
    }
}

fn next_view(next: NextAction) -> WorkflowNextView {
    match next {
        NextAction::Unblock { action } => WorkflowNextView::Unblock {
            action: blocked_action_view(action),
        },
        NextAction::AwaitPlacement { position } => WorkflowNextView::AwaitPlacement { position },
        NextAction::OwnerCompletes { position } => WorkflowNextView::OwnerCompletes { position },
        NextAction::InProgress { position, session } => WorkflowNextView::InProgress {
            position,
            session: session.map(|session| session.as_str().to_owned()),
        },
        NextAction::Nothing => WorkflowNextView::Nothing,
    }
}

fn status_view(status: RunStatus) -> WorkflowStatusView {
    match status {
        RunStatus::Running => WorkflowStatusView::Running,
        RunStatus::Delivered => WorkflowStatusView::Delivered,
        RunStatus::Cancelled => WorkflowStatusView::Cancelled,
    }
}

fn step_status_view(status: StepStatus) -> WorkflowStepStatusView {
    match status {
        StepStatus::Pending => WorkflowStepStatusView::Pending,
        StepStatus::Ready => WorkflowStepStatusView::Ready,
        StepStatus::Active => WorkflowStepStatusView::Active,
        StepStatus::Done => WorkflowStepStatusView::Done,
    }
}

fn internal(error: &dyn std::fmt::Display) -> Failed {
    tracing::error!(%error, "a workflows request failed");
    Failed(ProtocolError::new(
        ErrorCode::Internal,
        "the daemon failed; the request may or may not have taken effect",
    ))
}

impl From<Refusal> for Failed {
    fn from(refusal: Refusal) -> Failed {
        let code = match &refusal {
            Refusal::NotAuthorized(_) | Refusal::OwnerOnly { .. } => ErrorCode::NotAuthorized,
            Refusal::InvalidRequest { .. } | Refusal::UnknownPreset { .. } => {
                ErrorCode::InvalidArgument
            }
            Refusal::NotFound { .. } | Refusal::ItemNotFound { .. } => ErrorCode::NotFound,
            Refusal::ItemNotOpen { .. }
            | Refusal::AlreadyRunning { .. }
            | Refusal::NotRunning { .. }
            | Refusal::Blocked { .. }
            | Refusal::WrongStep { .. }
            | Refusal::AlreadyDoing { .. }
            | Refusal::NotDoing { .. }
            | Refusal::RequirementsUnmet { .. } => ErrorCode::Conflict,
            Refusal::MissingCapabilities { .. } => ErrorCode::Refused,
        };
        let (by, action) = refusal.unblock();
        refused(code, refusal.to_string(), &action, unblocked_by(by))
    }
}

impl From<cockpit_workflows::Error> for Failed {
    fn from(error: cockpit_workflows::Error) -> Failed {
        match error {
            cockpit_workflows::Error::Refused(refusal) => refusal.into(),
            error => internal(&error),
        }
    }
}
