//! Routes `cockpit todo` to `cockpit-tasks` and converts its types to the wire at this
//! boundary (#38). The owner acts on the host-only socket; an agent on the agent socket, where
//! `cockpit-agent-channel` identifies the sender and its working directory names its project.
//! The daemon checks that every project named is registered and reads the owner's `[tasks]`
//! settings; the rules live in `cockpit-tasks`.

use std::sync::Arc;

use cockpit_protocol::{
    Command, ErrorCode, HexBytes, ProtocolError, Reply, TaskDraftTargetView, TaskDraftView,
    TaskImageView, TaskKindView, TaskNextView, TaskStatusView, TaskView, TaskWaitView, Unblock,
    UnblockedBy,
};
use cockpit_tasks::{
    Actor, Draft, DraftTarget, Edit, ImageInfo, Item, ItemId, Kind, ListFilter, NewItem,
    NextAction, Priority, Refusal, Settings, Status,
};
use cockpit_turso::sql::Connection;
use cockpit_turso::{Database, WriteTransaction};

use crate::approvals::{Answer, Failed, Sender, job, now};
use crate::routes::{Peer, Route, WorkClass};

/// The route for a task command; `routes.rs` sends only those here, after the socket rules.
/// Agent requests run in the bulk lane like every agent request (#32), and so do image
/// transfers.
pub(crate) fn route(
    command: Command,
    peer: Peer,
    database: Arc<Database>,
    environment: cockpit_config::Environment,
) -> Route {
    let class = match (&peer, &command) {
        (Peer::Agent { .. }, _)
        | (Peer::Owner, Command::TaskImageAdd { .. } | Command::TaskImage { .. }) => {
            WorkClass::Bulk
        }
        (Peer::Owner, _) => WorkClass::Control,
    };
    let work = async move {
        let sender = match peer {
            Peer::Agent { connector } => Some(
                Sender::once_launched(connector, &database, launch_token(&command).as_deref())
                    .await?,
            ),
            Peer::Owner => None,
        };
        // The owner has no launch token to bind: reads need no write lock (#164).
        if sender.is_none() && is_read(&command) {
            let read = database.read().await.map_err(|error| internal(&error))?;
            return read_reply(&read, &Actor::Owner, command).await;
        }
        // Only a session's filings are bounded by the owner's settings.
        let settings = match (&sender, &command) {
            (
                Some(_),
                Command::TaskAdd { .. }
                | Command::TaskPropose { .. }
                | Command::TaskReportBlocker { .. },
            ) => settings(environment).await?,
            // Read by no other request.
            _ => Settings {
                blocker_project: None,
                cross_project_open_max: cockpit_config::CROSS_PROJECT_OPEN_MAX,
            },
        };
        let write = database.write().await.map_err(|error| internal(&error))?;
        let actor = match &sender {
            None => Actor::Owner,
            Some(sender) => {
                let session = sender.identify(&write, launch_token(&command)).await?;
                let project = sender.project(&write).await?.map(|project| project.name);
                Actor::Session { session, project }
            }
        };
        let reply = if is_read(&command) {
            read_reply(&write, &actor, command).await?
        } else {
            dispatch(&write, &actor, &settings, command).await?
        };
        write.commit().await.map_err(|error| internal(&error))?;
        Ok(reply)
    };
    Route::Run {
        class,
        job: job(work),
    }
}

async fn dispatch(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    settings: &Settings,
    command: Command,
) -> Answer {
    let now = now();
    let item = match command {
        Command::TaskAdd {
            project,
            title,
            notes,
            item_kind,
            priority,
            after,
            ..
        } => {
            registered(write, project.as_deref()).await?;
            let new = NewItem {
                project,
                title,
                notes,
                kind: item_kind.map(kind).transpose()?.unwrap_or(Kind::Task),
                priority: priority
                    .map(priority_of)
                    .transpose()?
                    .unwrap_or(Priority::NORMAL),
                after: after.into_iter().map(ItemId).collect(),
            };
            cockpit_tasks::add(write, actor, settings, new, now).await?
        }
        Command::TaskPropose {
            project,
            title,
            why,
            notes,
            item_kind,
            ..
        } => {
            registered(write, project.as_deref()).await?;
            let new = NewItem {
                project,
                title,
                notes,
                kind: item_kind.map(kind).transpose()?.unwrap_or(Kind::Task),
                priority: Priority::NORMAL,
                after: Vec::new(),
            };
            cockpit_tasks::propose(write, actor, settings, new, &why, now).await?
        }
        Command::TaskReportBlocker {
            title, why, notes, ..
        } => {
            registered(write, settings.blocker_project.as_deref()).await?;
            cockpit_tasks::report_blocker(write, actor, settings, &title, &why, &notes, now).await?
        }
        Command::TaskEdit {
            item,
            intent_revision,
            title,
            notes,
            item_kind,
        } => {
            let edit = Edit {
                title,
                notes,
                kind: item_kind.map(kind).transpose()?,
            };
            cockpit_tasks::edit(write, actor, ItemId(item), intent_revision, edit, now).await?
        }
        Command::TaskPriority { item, priority } => {
            let priority = priority_of(priority)?;
            cockpit_tasks::set_priority(write, actor, ItemId(item), priority, now).await?
        }
        Command::TaskQueue { item, queued } => {
            cockpit_tasks::set_queued(write, actor, ItemId(item), queued, now).await?
        }
        Command::TaskApprove { item } => {
            cockpit_tasks::approve(write, actor, ItemId(item), now).await?
        }
        Command::TaskDecline { item } => {
            cockpit_tasks::decline(write, actor, ItemId(item), now).await?
        }
        Command::TaskStart { item, .. } => {
            cockpit_tasks::start(write, actor, ItemId(item), now).await?
        }
        Command::TaskDone { item, summary, .. } => {
            cockpit_tasks::finish(write, actor, ItemId(item), summary.as_deref(), now).await?
        }
        Command::TaskReopen { item } => {
            cockpit_tasks::reopen(write, actor, ItemId(item), now).await?
        }
        Command::TaskWait { item, on } => {
            let on = on.into_iter().map(ItemId).collect();
            cockpit_tasks::wait(write, actor, ItemId(item), on, now).await?
        }
        Command::TaskUnwait { item, on } => {
            let on = on.into_iter().map(ItemId).collect();
            cockpit_tasks::unwait(write, actor, ItemId(item), on, now).await?
        }
        Command::TaskDelete { item, .. } => {
            cockpit_tasks::delete(write, actor, ItemId(item)).await?;
            return Ok(Reply::TaskDeleted { item });
        }
        Command::TaskImageAdd { item, data } => {
            let image = cockpit_tasks::add_image(write, actor, ItemId(item), data.0, now).await?;
            return Ok(Reply::TaskImageAdded {
                item,
                image: image_view(&image),
            });
        }
        Command::TaskDraftSave {
            target,
            title,
            notes,
            base_intent_revision,
        } => {
            if let TaskDraftTargetView::New { project } = &target {
                registered(write, Some(project)).await?;
            }
            let draft = cockpit_tasks::save_draft(
                write,
                actor,
                &draft_target(target),
                &title,
                &notes,
                base_intent_revision,
                now,
            )
            .await?;
            return Ok(Reply::TaskDraft {
                draft: draft_view(&draft),
            });
        }
        Command::TaskDraftDiscard { target } => {
            // No registration check: a project removed since keeps its draft discardable.
            cockpit_tasks::discard_draft(write, actor, &draft_target(target)).await?;
            return Ok(Reply::TaskDraftDiscarded);
        }
        other => unreachable!("routes.rs sends only task commands here, not {other:?}"),
    };
    Ok(Reply::Task {
        task: Box::new(task_view(item)),
    })
}

fn is_read(command: &Command) -> bool {
    matches!(
        command,
        Command::TaskList { .. }
            | Command::TaskShow { .. }
            | Command::TaskImage { .. }
            | Command::TaskDrafts
    )
}

async fn read_reply(connection: &Connection, actor: &Actor, command: Command) -> Answer {
    Ok(match command {
        Command::TaskList { project, all, .. } => {
            let filter = ListFilter {
                project,
                include_closed: all,
            };
            let items = cockpit_tasks::list(connection, actor, &filter).await?;
            Reply::Tasks {
                tasks: items.into_iter().map(task_view).collect(),
            }
        }
        Command::TaskShow { item, .. } => Reply::Task {
            task: Box::new(task_view(
                cockpit_tasks::item(connection, actor, ItemId(item)).await?,
            )),
        },
        Command::TaskImage { item, number } => {
            let image = cockpit_tasks::image(connection, actor, ItemId(item), number).await?;
            Reply::TaskImage {
                item,
                image: TaskImageView {
                    number: image.number,
                    media_type: image.media_type.to_owned(),
                    bytes: image.bytes.len() as u64,
                },
                data: HexBytes(image.bytes),
            }
        }
        Command::TaskDrafts => {
            let drafts = cockpit_tasks::drafts(connection, actor).await?;
            Reply::TaskDrafts {
                drafts: drafts.iter().map(draft_view).collect(),
            }
        }
        other => unreachable!("is_read admits only reads, not {other:?}"),
    })
}

/// The launch token a command from a new launch carries.
fn launch_token(command: &Command) -> Option<String> {
    match command {
        Command::TaskAdd { launch_token, .. }
        | Command::TaskPropose { launch_token, .. }
        | Command::TaskReportBlocker { launch_token, .. }
        | Command::TaskList { launch_token, .. }
        | Command::TaskShow { launch_token, .. }
        | Command::TaskStart { launch_token, .. }
        | Command::TaskDone { launch_token, .. }
        | Command::TaskDelete { launch_token, .. } => launch_token.clone(),
        _ => None,
    }
}

/// The owner's `[tasks]` settings, read before the command acts: a broken owner config
/// refuses the filing instead of applying a default the owner did not choose.
async fn settings(environment: cockpit_config::Environment) -> Result<Settings, Failed> {
    let loaded = tokio::task::spawn_blocking(move || cockpit_config::load(&environment, None, &[]))
        .await
        .map_err(|error| internal(&error))?;
    let config = loaded.map_err(|error| {
        refused(
            ErrorCode::Refused,
            format!("the owner configuration cannot be read: {error}"),
            "fix the owner configuration file",
            UnblockedBy::Owner,
        )
    })?;
    Ok(Settings {
        blocker_project: config.tasks_blocker_project.value,
        cross_project_open_max: config.tasks_cross_project_open_max.value,
    })
}

/// Refuses a project name that is not a registered project.
async fn registered(connection: &Connection, project: Option<&str>) -> Result<(), Failed> {
    let Some(project) = project else {
        return Ok(());
    };
    match cockpit_projects::project_named(connection, project).await {
        Ok(_) => Ok(()),
        Err(cockpit_projects::Error::UnknownProject { .. }) => Err(refused(
            ErrorCode::NotFound,
            format!("no registered project {project}"),
            "name a project `cockpit project list` shows",
            UnblockedBy::Agent,
        )),
        Err(error) => Err(internal(&error)),
    }
}

fn draft_target(target: TaskDraftTargetView) -> DraftTarget {
    match target {
        TaskDraftTargetView::New { project } => DraftTarget::New { project },
        TaskDraftTargetView::Item { item } => DraftTarget::Item(ItemId(item)),
    }
}

fn kind(view: TaskKindView) -> Result<Kind, Failed> {
    Ok(match view {
        TaskKindView::Task => Kind::Task,
        TaskKindView::Feature => Kind::Feature,
        TaskKindView::Bug => Kind::Bug,
        TaskKindView::Improvement => Kind::Improvement,
        TaskKindView::Chore => Kind::Chore,
        TaskKindView::Research => Kind::Research,
        TaskKindView::Unknown => {
            return Err(refused(
                ErrorCode::InvalidArgument,
                "item_kind: unknown kind".to_owned(),
                &format!("use one of {}", TaskKindView::known_names()),
                UnblockedBy::Agent,
            ));
        }
    })
}

fn priority_of(level: i64) -> Result<Priority, Failed> {
    Priority::new(level).ok_or_else(|| {
        refused(
            ErrorCode::InvalidArgument,
            format!("priority: {level} is not from 1 to 6"),
            "use a priority from 1 (low) to 6; 2 is normal",
            UnblockedBy::Agent,
        )
    })
}

fn refused(code: ErrorCode, message: String, action: &str, by: UnblockedBy) -> Failed {
    Failed(ProtocolError {
        code,
        message,
        unblock: Some(Unblock {
            action: action.to_owned(),
            by,
        }),
    })
}

fn internal(error: &dyn std::fmt::Display) -> Failed {
    tracing::error!(%error, "a tasks request failed");
    Failed(ProtocolError::new(
        ErrorCode::Internal,
        "the daemon failed; the request may or may not have taken effect",
    ))
}

impl From<Refusal> for Failed {
    fn from(refusal: Refusal) -> Failed {
        let code = match &refusal {
            Refusal::NotAuthorized(_) | Refusal::OwnerOnly { .. } => ErrorCode::NotAuthorized,
            Refusal::InvalidRequest { .. }
            | Refusal::InvalidImage { .. }
            | Refusal::Cycle { .. } => ErrorCode::InvalidArgument,
            Refusal::NotFound { .. } => ErrorCode::NotFound,
            Refusal::StaleRevision { .. } => ErrorCode::StaleRevision,
            Refusal::WrongStatus { .. }
            | Refusal::Waiting { .. }
            | Refusal::Declined { .. }
            | Refusal::CapReached { .. }
            | Refusal::WaitedOn { .. }
            | Refusal::NotDoing { .. } => ErrorCode::Conflict,
            Refusal::NoBlockerProject | Refusal::NoProject => ErrorCode::Refused,
        };
        let (by, action) = refusal.unblock();
        refused(
            code,
            refusal.to_string(),
            &action,
            match by {
                cockpit_tasks::UnblockedBy::Owner => UnblockedBy::Owner,
                cockpit_tasks::UnblockedBy::Agent => UnblockedBy::Agent,
            },
        )
    }
}

impl From<cockpit_tasks::Error> for Failed {
    fn from(error: cockpit_tasks::Error) -> Failed {
        match error {
            cockpit_tasks::Error::Refused(refusal) => refusal.into(),
            error => internal(&error),
        }
    }
}

fn task_view(item: Item) -> TaskView {
    let next = match item.next_action() {
        NextAction::DecideProposal => TaskNextView::DecideProposal,
        NextAction::FinishDependencies { items } => TaskNextView::FinishDependencies {
            items: items.iter().map(|item| item.0).collect(),
        },
        NextAction::Queue => TaskNextView::Queue,
        NextAction::AwaitPlacement => TaskNextView::AwaitPlacement,
        NextAction::InProgress { session } => TaskNextView::InProgress {
            session: session.map(|session| session.as_str().to_owned()),
        },
        NextAction::Nothing => TaskNextView::Nothing,
    };
    TaskView {
        id: item.id.0,
        project: item.project,
        title: item.title,
        notes: item.notes,
        kind: kind_view(item.kind),
        priority: item.priority.level(),
        status: status_view(item.status),
        is_blocker_report: item.is_blocker_report,
        why: item.why,
        filed_by: item
            .filed_by
            .as_ref()
            .map(|session| session.as_str().to_owned()),
        filed_from: item.filed_from,
        doing_session: item
            .doing_session
            .as_ref()
            .map(|session| session.as_str().to_owned()),
        summary: item.summary,
        is_queued: item.is_queued,
        waits: item
            .waits
            .into_iter()
            .map(|wait| TaskWaitView {
                item: wait.item.0,
                title: wait.title,
                status: status_view(wait.status),
                summary: wait.summary,
            })
            .collect(),
        images: item.images.iter().map(image_view).collect(),
        intent_revision: item.intent_revision,
        state_revision: item.state_revision,
        created_at: item.created_at,
        changed_at: item.changed_at,
        next,
    }
}

fn kind_view(kind: Kind) -> TaskKindView {
    match kind {
        Kind::Task => TaskKindView::Task,
        Kind::Feature => TaskKindView::Feature,
        Kind::Bug => TaskKindView::Bug,
        Kind::Improvement => TaskKindView::Improvement,
        Kind::Chore => TaskKindView::Chore,
        Kind::Research => TaskKindView::Research,
    }
}

fn status_view(status: Status) -> TaskStatusView {
    match status {
        Status::Proposed => TaskStatusView::Proposed,
        Status::Todo => TaskStatusView::Todo,
        Status::Doing => TaskStatusView::Doing,
        Status::Done => TaskStatusView::Done,
        Status::Declined => TaskStatusView::Declined,
    }
}

fn image_view(image: &ImageInfo) -> TaskImageView {
    TaskImageView {
        number: image.number,
        media_type: image.media_type.to_owned(),
        bytes: image.bytes,
    }
}

fn draft_view(draft: &Draft) -> TaskDraftView {
    TaskDraftView {
        target: match &draft.target {
            DraftTarget::New { project } => TaskDraftTargetView::New {
                project: project.clone(),
            },
            DraftTarget::Item(item) => TaskDraftTargetView::Item { item: item.0 },
        },
        title: draft.title.clone(),
        notes: draft.notes.clone(),
        base_intent_revision: draft.base_intent_revision,
        is_stale: draft.is_stale,
        saved_at: draft.saved_at,
    }
}
