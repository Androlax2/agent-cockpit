//! Items and their status.
//!
//! A session files an item as `todo` (`add`), or as `proposed` (`propose`), which becomes
//! work only when the owner approves it (`todo`) and is otherwise `declined`, a title that
//! project's sessions cannot file again. `start` moves `todo` to `doing`, refused while the
//! item waits on unfinished items; `finish` moves it to `done` with the summary the items
//! waiting on it read; the owner's `reopen` moves it back to `todo`.
//!
//! The owner may do everything. A session needs `change-state` over itself, granted at
//! launch, and acts only on its own project's items and those it filed; filing in another
//! project's list counts against that pair's cap.

use std::collections::HashMap;

use cockpit_authorization::{Permission, Scope, SessionId};
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::error::invalid;
use crate::text::{self, NOTES_MAX_CHARS, SUMMARY_MAX_CHARS, WHY_MAX_CHARS};
use crate::{
    Actor, Edit, Error, Item, ItemId, Kind, NewItem, Priority, Refusal, SessionWork, Settings,
    Status, Wait,
};

/// The columns `item_from` reads, in its order.
const SELECT_ITEMS: &str = "SELECT id, project, title, notes, kind, priority, status,
        is_blocker_report, why, filed_by, filed_from, doing_session, summary, is_queued,
        intent_revision, state_revision, created_at, changed_at, finished_by
     FROM tasks_items";

/// What `list` returns.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ListFilter {
    /// One project's list; every visible item without it.
    pub project: Option<String>,
    /// Done and declined items too.
    pub include_closed: bool,
}

/// How an item enters the list.
struct Filing<'a> {
    status: Status,
    is_queued: bool,
    is_blocker_report: bool,
    why: Option<&'a str>,
}

/// Files a `todo` item. The owner names the project. A session files in its own project's
/// list, or in another one's, where the item is queued at once (handed across) and counts
/// against the cap.
pub async fn add(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    settings: &Settings,
    new: NewItem,
    now: i64,
) -> Result<Item, Error> {
    let project = target_project(actor, new.project.as_deref())?;
    let is_handed_across = is_elsewhere(actor, &project);
    let filing = Filing {
        status: Status::Todo,
        is_queued: is_handed_across,
        is_blocker_report: false,
        why: None,
    };
    file(write, actor, settings, &project, new, filing, now).await
}

/// Files a session's suggestion as `proposed`: the owner approves or declines it.
pub async fn propose(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    settings: &Settings,
    new: NewItem,
    why: &str,
    now: i64,
) -> Result<Item, Error> {
    if *actor == Actor::Owner {
        return Err(invalid("proposal", "the owner adds items; sessions propose them").into());
    }
    let why = text::required_body("why", why, WHY_MAX_CHARS)?;
    let project = target_project(actor, new.project.as_deref())?;
    let filing = Filing {
        status: Status::Proposed,
        is_queued: false,
        is_blocker_report: false,
        why: Some(&why),
    };
    file(write, actor, settings, &project, new, filing, now).await
}

/// Files what a session could not do because of the setup in the owner's configured blocker
/// project, queued for its sessions.
pub async fn report_blocker(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    settings: &Settings,
    title: &str,
    why: &str,
    notes: &str,
    now: i64,
) -> Result<Item, Error> {
    if *actor == Actor::Owner {
        return Err(invalid("blocker", "blocker reports are sessions'").into());
    }
    let why = text::required_body("why", why, WHY_MAX_CHARS)?;
    let project = settings
        .blocker_project
        .clone()
        .ok_or(Refusal::NoBlockerProject)?;
    let new = NewItem {
        project: None,
        title: title.to_owned(),
        notes: notes.to_owned(),
        kind: Kind::Task,
        priority: Priority::NORMAL,
        after: Vec::new(),
    };
    let filing = Filing {
        status: Status::Todo,
        is_queued: true,
        is_blocker_report: true,
        why: Some(&why),
    };
    file(write, actor, settings, &project, new, filing, now).await
}

async fn file(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    settings: &Settings,
    project: &str,
    new: NewItem,
    filing: Filing<'_>,
    now: i64,
) -> Result<Item, Error> {
    let title = text::title("title", &new.title)?;
    let notes = text::body("notes", &new.notes, NOTES_MAX_CHARS)?;
    let title_key = text::title_key(&title);
    authorize(write, actor).await?;
    let (filed_by, filed_from) = match actor {
        Actor::Owner => (None, None),
        Actor::Session { session, project } => (Some(session.as_str()), project.as_deref()),
    };
    if filed_by.is_some() {
        if was_declined(write, project, &title_key).await? {
            return Err(Refusal::Declined {
                project: project.to_owned(),
            }
            .into());
        }
        // A blocker report goes where the owner configured; the cap is for choices sessions
        // make about another project's list, so a session outside every project, which the
        // cap cannot count, files nothing else.
        if !filing.is_blocker_report {
            let from = filed_from.ok_or(Refusal::NoProject)?;
            if from != project {
                check_cap(write, settings, from, project).await?;
            }
        }
    }
    for waited in &new.after {
        waitable(write, actor, *waited).await?;
    }
    // AUTOINCREMENT: a deleted item's id is never given to another, so a `Todo:` trailer or
    // a command naming it cannot reach a different item.
    let id = write
        .prepare(
            "INSERT INTO tasks_items (project, title, title_key, notes, kind, priority,
                status, is_blocker_report, why, filed_by, filed_from, is_queued,
                intent_revision, state_revision, created_at, changed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 1, 1, ?13, ?13)
             RETURNING id",
        )
        .await?
        .query_row((
            project,
            title.as_str(),
            title_key.as_str(),
            notes.as_str(),
            new.kind.as_str(),
            new.priority.level(),
            filing.status.as_str(),
            i64::from(filing.is_blocker_report),
            filing.why,
            filed_by,
            filed_from,
            i64::from(filing.is_queued),
            now,
        ))
        .await?
        .get::<i64>(0)?;
    for waited in dedup(new.after) {
        insert_wait(write, ItemId(id), waited).await?;
    }
    required(write, ItemId(id)).await
}

/// The project a filing goes to: the one named, else the session's own.
fn target_project(actor: &Actor, named: Option<&str>) -> Result<String, Refusal> {
    match (named, actor) {
        (Some(""), _) => Err(invalid("project", "is empty")),
        (Some(named), _) => Ok(named.to_owned()),
        (
            None,
            Actor::Session {
                project: Some(own), ..
            },
        ) => Ok(own.clone()),
        (None, Actor::Session { project: None, .. }) => Err(Refusal::NoProject),
        (None, Actor::Owner) => Err(invalid("project", "name the project the item is for")),
    }
}

fn is_elsewhere(actor: &Actor, project: &str) -> bool {
    match actor {
        Actor::Owner => false,
        Actor::Session { project: own, .. } => own.as_deref() != Some(project),
    }
}

async fn was_declined(
    connection: &Connection,
    project: &str,
    title_key: &str,
) -> Result<bool, Error> {
    let mut rows = connection
        .query(
            "SELECT 1 FROM tasks_items WHERE status = 'declined' AND project = ?1
                AND title_key = ?2 LIMIT 1",
            (project, title_key),
        )
        .await?;
    Ok(rows.next().await?.is_some())
}

/// Refuses a filing from `from` into `project` once that pair has the cap of open items.
async fn check_cap(
    connection: &Connection,
    settings: &Settings,
    from: &str,
    project: &str,
) -> Result<(), Error> {
    let open = connection
        .prepare(
            "SELECT count(*) FROM tasks_items
             WHERE filed_from = ?1 AND project = ?2 AND is_blocker_report = 0
                AND status IN ('proposed', 'todo', 'doing')",
        )
        .await?
        .query_row((from, project))
        .await?
        .get::<i64>(0)?;
    let open = u64::try_from(open).unwrap_or(0);
    if open >= u64::from(settings.cross_project_open_max) {
        return Err(Refusal::CapReached {
            project: project.to_owned(),
            open,
            cap: settings.cross_project_open_max,
        }
        .into());
    }
    Ok(())
}

/// Every item the actor may see, highest priority first within each status, in the order
/// work moves: doing, todo, proposed, then done and declined.
pub async fn list(
    connection: &Connection,
    actor: &Actor,
    filter: &ListFilter,
) -> Result<Vec<Item>, Error> {
    authorize(connection, actor).await?;
    let (session, own_project) = match actor {
        Actor::Owner => (None, None),
        Actor::Session { session, project } => (Some(session), project.as_deref()),
    };
    let mut rows = connection
        .query(
            &format!(
                "{SELECT_ITEMS}
                 WHERE (?1 IS NULL OR project = ?1)
                    AND (?2 = 1 OR status IN ('proposed', 'todo', 'doing'))
                    AND (?3 = 0 OR project IS ?4 OR filed_by IS ?5)
                 ORDER BY CASE status WHEN 'doing' THEN 0 WHEN 'todo' THEN 1
                    WHEN 'proposed' THEN 2 WHEN 'done' THEN 3 ELSE 4 END,
                    priority DESC, id"
            ),
            (
                filter.project.as_deref(),
                i64::from(filter.include_closed),
                i64::from(session.is_some()),
                own_project,
                session.map(SessionId::as_str),
            ),
        )
        .await?;
    let mut items = Vec::new();
    while let Some(row) = rows.next().await? {
        items.push(item_from(&row)?);
    }
    for item in &mut items {
        complete(connection, item).await?;
    }
    Ok(items)
}

/// One item the actor may see.
pub async fn item(connection: &Connection, actor: &Actor, id: ItemId) -> Result<Item, Error> {
    accessible(connection, actor, id).await
}

/// What the list says about each session's work, for the owner's view of sessions (#157):
/// how many items it is doing, and when it last finished one it was doing (#226). Sessions
/// with neither are absent.
pub async fn session_work(
    connection: &Connection,
) -> Result<HashMap<SessionId, SessionWork>, Error> {
    let mut work: HashMap<SessionId, SessionWork> = HashMap::new();
    let doing = "SELECT doing_session, count(*) FROM tasks_items
        WHERE doing_session IS NOT NULL GROUP BY doing_session";
    for (session, count) in counts_by_session(connection, doing).await? {
        work.entry(session).or_default().doing = count;
    }
    let finished = "SELECT finished_by, max(finished_at) FROM tasks_items
        WHERE finished_by IS NOT NULL AND status = 'done' GROUP BY finished_by";
    let mut rows = connection.query(finished, ()).await?;
    while let Some(row) = rows.next().await? {
        let session = SessionId::new(row.get::<String>(0)?)
            .map_err(|error| invalid_row("tasks_items", error.to_string()))?;
        work.entry(session).or_default().last_finished_at = Some(row.get::<i64>(1)?);
    }
    Ok(work)
}

/// The rows of `query`: a session column, then a count.
async fn counts_by_session(
    connection: &Connection,
    query: &str,
) -> Result<Vec<(SessionId, u32)>, Error> {
    let mut rows = connection.query(query, ()).await?;
    let mut counts = Vec::new();
    while let Some(row) = rows.next().await? {
        let session = SessionId::new(row.get::<String>(0)?)
            .map_err(|error| invalid_row("tasks_items", error.to_string()))?;
        let count = u32::try_from(row.get::<i64>(1)?)
            .map_err(|error| invalid_row("tasks_items", error.to_string()))?;
        counts.push((session, count));
    }
    Ok(counts)
}

/// The owner's change to what is asked. `expected_intent_revision`, when given, must be the
/// current one, so an edit made on an older reading is refused instead of overwriting.
pub async fn edit(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: ItemId,
    expected_intent_revision: Option<i64>,
    edit: Edit,
    now: i64,
) -> Result<Item, Error> {
    owner_only(actor, "edit an item")?;
    let item = required(write, id).await?;
    if let Some(expected) = expected_intent_revision
        && expected != item.intent_revision
    {
        return Err(Refusal::StaleRevision {
            current: item.intent_revision,
        }
        .into());
    }
    let title = edit
        .title
        .map(|title| text::title("title", &title))
        .transpose()?
        .unwrap_or(item.title);
    let notes = edit
        .notes
        .map(|notes| text::body("notes", &notes, NOTES_MAX_CHARS))
        .transpose()?
        .unwrap_or(item.notes);
    let kind = edit.kind.unwrap_or(item.kind);
    write
        .execute(
            "UPDATE tasks_items SET title = ?2, title_key = ?3, notes = ?4, kind = ?5,
                intent_revision = intent_revision + 1, changed_at = ?6
             WHERE id = ?1",
            (
                id.0,
                title.as_str(),
                text::title_key(&title).as_str(),
                notes.as_str(),
                kind.as_str(),
                now,
            ),
        )
        .await?;
    required(write, id).await
}

pub async fn set_priority(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: ItemId,
    priority: Priority,
    now: i64,
) -> Result<Item, Error> {
    owner_only(actor, "change a priority")?;
    required(write, id).await?;
    write
        .execute(
            "UPDATE tasks_items SET priority = ?2, state_revision = state_revision + 1,
                changed_at = ?3
             WHERE id = ?1",
            (id.0, priority.level(), now),
        )
        .await?;
    required(write, id).await
}

/// The owner makes a `todo` item available for a session to take, or takes it back.
pub async fn set_queued(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: ItemId,
    is_queued: bool,
    now: i64,
) -> Result<Item, Error> {
    owner_only(actor, "queue an item")?;
    let item = required(write, id).await?;
    let action = if is_queued {
        "be queued"
    } else {
        "be unqueued"
    };
    in_status(&item, &[Status::Todo], action)?;
    write
        .execute(
            "UPDATE tasks_items SET is_queued = ?2, state_revision = state_revision + 1,
                changed_at = ?3
             WHERE id = ?1",
            (id.0, i64::from(is_queued), now),
        )
        .await?;
    required(write, id).await
}

/// The owner turns a proposal into work.
pub async fn approve(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: ItemId,
    now: i64,
) -> Result<Item, Error> {
    owner_only(actor, "approve a proposal")?;
    move_status(
        write,
        id,
        &[Status::Proposed],
        Status::Todo,
        "be approved",
        now,
    )
    .await
}

/// The owner declines a proposal; its title stays declined in its project.
pub async fn decline(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: ItemId,
    now: i64,
) -> Result<Item, Error> {
    owner_only(actor, "decline a proposal")?;
    refuse_while_waited_on(write, id).await?;
    move_status(
        write,
        id,
        &[Status::Proposed],
        Status::Declined,
        "be declined",
        now,
    )
    .await
}

/// `todo` to `doing`, refused while it waits on unfinished items. A session starts only its
/// own project's items.
pub async fn start(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: ItemId,
    now: i64,
) -> Result<Item, Error> {
    let item = accessible(write, actor, id).await?;
    let doing_session = match actor {
        Actor::Owner => None,
        Actor::Session { session, project } => {
            if project.as_deref() != Some(item.project.as_str()) {
                return Err(Refusal::NotFound { item: id }.into());
            }
            Some(session.as_str())
        }
    };
    in_status(&item, &[Status::Todo], "start")?;
    refuse_while_waiting(&item)?;
    write
        .execute(
            "UPDATE tasks_items SET status = 'doing', doing_session = ?2, is_queued = 0,
                state_revision = state_revision + 1, changed_at = ?3
             WHERE id = ?1",
            (id.0, doing_session, now),
        )
        .await?;
    required(write, id).await
}

/// To `done`, with what was done for the items waiting on it. The owner finishes any open
/// work; a session only the item it is doing.
pub async fn finish(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: ItemId,
    summary: Option<&str>,
    now: i64,
) -> Result<Item, Error> {
    let summary = summary
        .map(|summary| text::body("summary", summary, SUMMARY_MAX_CHARS))
        .transpose()?
        .filter(|summary| !summary.trim().is_empty());
    let item = accessible(write, actor, id).await?;
    match actor {
        Actor::Owner => in_status(&item, &[Status::Todo, Status::Doing], "be finished")?,
        Actor::Session { session, .. } => {
            if item.status != Status::Doing || item.doing_session.as_ref() != Some(session) {
                return Err(Refusal::NotDoing { item: id }.into());
            }
        }
    }
    refuse_while_waiting(&item)?;
    write
        .execute(
            "UPDATE tasks_items SET status = 'done', finished_by = doing_session, finished_at = ?3,
                doing_session = NULL, is_queued = 0, summary = ?2, state_revision = state_revision + 1, changed_at = ?3
             WHERE id = ?1",
            (id.0, summary, now),
        )
        .await?;
    required(write, id).await
}

/// The owner puts started or finished work back to `todo`; its summary goes.
pub async fn reopen(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: ItemId,
    now: i64,
) -> Result<Item, Error> {
    owner_only(actor, "reopen an item")?;
    let item = required(write, id).await?;
    in_status(&item, &[Status::Doing, Status::Done], "be reopened")?;
    write
        .execute(
            "UPDATE tasks_items SET status = 'todo', doing_session = NULL, finished_by = NULL,
                finished_at = NULL, summary = NULL, state_revision = state_revision + 1, changed_at = ?2
             WHERE id = ?1",
            (id.0, now),
        )
        .await?;
    required(write, id).await
}

/// The owner makes `id` wait on `on` before it can start. A wait that would make an item wait
/// on itself, through any chain, is refused.
pub async fn wait(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: ItemId,
    on: Vec<ItemId>,
    now: i64,
) -> Result<Item, Error> {
    owner_only(actor, "change what an item waits on")?;
    if on.is_empty() {
        return Err(invalid("after", "name at least one item").into());
    }
    required(write, id).await?;
    for waited in dedup(on) {
        waitable(write, actor, waited).await?;
        if waited == id || reaches(write, waited, id).await? {
            return Err(Refusal::Cycle {
                item: id,
                on: waited,
            }
            .into());
        }
        insert_wait(write, id, waited).await?;
    }
    bump_intent(write, id, now).await?;
    required(write, id).await
}

/// The owner removes waits; an item it did not wait on is ignored.
pub async fn unwait(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    id: ItemId,
    on: Vec<ItemId>,
    now: i64,
) -> Result<Item, Error> {
    owner_only(actor, "change what an item waits on")?;
    if on.is_empty() {
        return Err(invalid("after", "name at least one item").into());
    }
    required(write, id).await?;
    for waited in on {
        write
            .execute(
                "DELETE FROM tasks_waits WHERE item = ?1 AND waits_on = ?2",
                (id.0, waited.0),
            )
            .await?;
    }
    bump_intent(write, id, now).await?;
    required(write, id).await
}

/// Removes an item with its waits, images and draft. The owner deletes any item no open item
/// waits on; a session only what it filed and nobody started.
pub async fn delete(write: &WriteTransaction<'_>, actor: &Actor, id: ItemId) -> Result<(), Error> {
    let item = accessible(write, actor, id).await?;
    if let Actor::Session { session, .. } = actor {
        if item.filed_by.as_ref() != Some(session) {
            return Err(Refusal::OwnerOnly {
                action: "delete an item the session did not file",
            }
            .into());
        }
        in_status(&item, &[Status::Proposed, Status::Todo], "be deleted")?;
    }
    refuse_while_waited_on(write, id).await?;
    for statement in [
        "DELETE FROM tasks_waits WHERE item = ?1 OR waits_on = ?1",
        "DELETE FROM tasks_images WHERE item = ?1",
        "DELETE FROM tasks_drafts WHERE item = ?1",
        "DELETE FROM tasks_items WHERE id = ?1",
    ] {
        write.execute(statement, [id.0]).await?;
    }
    Ok(())
}

/// Deleting or declining an item open items wait on would leave them waiting for ever.
/// Starting or finishing an item before what it waits on is done would skip the order.
fn refuse_while_waiting(item: &Item) -> Result<(), Refusal> {
    let on = item.waiting_on();
    if on.is_empty() {
        return Ok(());
    }
    Err(Refusal::Waiting { item: item.id, on })
}

async fn refuse_while_waited_on(connection: &Connection, id: ItemId) -> Result<(), Error> {
    let waiting = open_waiting_on(connection, id).await?;
    if waiting.is_empty() {
        return Ok(());
    }
    Err(Refusal::WaitedOn {
        item: id,
        by: waiting,
    }
    .into())
}

async fn open_waiting_on(connection: &Connection, id: ItemId) -> Result<Vec<ItemId>, Error> {
    let mut rows = connection
        .query(
            "SELECT w.item FROM tasks_waits w JOIN tasks_items i ON i.id = w.item
             WHERE w.waits_on = ?1 AND i.status IN ('proposed', 'todo', 'doing')
             ORDER BY w.item",
            [id.0],
        )
        .await?;
    let mut waiting = Vec::new();
    while let Some(row) = rows.next().await? {
        waiting.push(ItemId(row.get(0)?));
    }
    Ok(waiting)
}

/// A session needs `change-state` over itself for anything on tasks; the owner holds every
/// permission.
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

pub(crate) fn owner_only(actor: &Actor, action: &'static str) -> Result<(), Refusal> {
    match actor {
        Actor::Owner => Ok(()),
        Actor::Session { .. } => Err(Refusal::OwnerOnly { action }),
    }
}

/// A session sees its own project's list and what it filed elsewhere.
fn is_visible(actor: &Actor, item: &Item) -> bool {
    match actor {
        Actor::Owner => true,
        Actor::Session { session, project } => {
            project.as_deref() == Some(item.project.as_str())
                || item.filed_by.as_ref() == Some(session)
        }
    }
}

/// The item a request acts on: the session's grant checked first, then whether it may see it.
async fn accessible(connection: &Connection, actor: &Actor, id: ItemId) -> Result<Item, Error> {
    authorize(connection, actor).await?;
    visible(connection, actor, id).await
}

async fn visible(connection: &Connection, actor: &Actor, id: ItemId) -> Result<Item, Error> {
    match fetch(connection, id).await? {
        Some(item) if is_visible(actor, &item) => Ok(item),
        _ => Err(Refusal::NotFound { item: id }.into()),
    }
}

fn in_status(item: &Item, allowed: &[Status], action: &'static str) -> Result<(), Refusal> {
    if allowed.contains(&item.status) {
        return Ok(());
    }
    Err(Refusal::WrongStatus {
        item: item.id,
        status: item.status,
        action,
    })
}

async fn move_status(
    write: &WriteTransaction<'_>,
    id: ItemId,
    from: &[Status],
    to: Status,
    action: &'static str,
    now: i64,
) -> Result<Item, Error> {
    let item = required(write, id).await?;
    in_status(&item, from, action)?;
    write
        .execute(
            "UPDATE tasks_items SET status = ?2, state_revision = state_revision + 1,
                changed_at = ?3
             WHERE id = ?1",
            (id.0, to.as_str(), now),
        )
        .await?;
    required(write, id).await
}

pub(crate) async fn bump_intent(
    write: &WriteTransaction<'_>,
    id: ItemId,
    now: i64,
) -> Result<(), Error> {
    write
        .execute(
            "UPDATE tasks_items SET intent_revision = intent_revision + 1, changed_at = ?2
             WHERE id = ?1",
            (id.0, now),
        )
        .await?;
    Ok(())
}

/// An item the actor may make another wait on: one it can see that was not declined.
async fn waitable(connection: &Connection, actor: &Actor, id: ItemId) -> Result<(), Error> {
    let item = visible(connection, actor, id).await?;
    if item.status == Status::Declined {
        return Err(Refusal::WrongStatus {
            item: id,
            status: item.status,
            action: "be waited on",
        }
        .into());
    }
    Ok(())
}

/// Whether `from` waits on `to`, directly or through other items.
async fn reaches(connection: &Connection, from: ItemId, to: ItemId) -> Result<bool, Error> {
    let mut rows = connection
        .query(
            "WITH RECURSIVE reached(id) AS (
                SELECT waits_on FROM tasks_waits WHERE item = ?1
                UNION
                SELECT w.waits_on FROM tasks_waits w JOIN reached r ON w.item = r.id
             )
             SELECT 1 FROM reached WHERE id = ?2 LIMIT 1",
            (from.0, to.0),
        )
        .await?;
    Ok(rows.next().await?.is_some())
}

async fn insert_wait(write: &WriteTransaction<'_>, id: ItemId, on: ItemId) -> Result<(), Error> {
    write
        .execute(
            "INSERT INTO tasks_waits (item, waits_on) SELECT ?1, ?2
             WHERE NOT EXISTS (SELECT 1 FROM tasks_waits WHERE item = ?1 AND waits_on = ?2)",
            (id.0, on.0),
        )
        .await?;
    Ok(())
}

fn dedup(mut ids: Vec<ItemId>) -> Vec<ItemId> {
    ids.sort();
    ids.dedup();
    ids
}

pub(crate) async fn fetch(connection: &Connection, id: ItemId) -> Result<Option<Item>, Error> {
    let mut rows = connection
        .query(&format!("{SELECT_ITEMS} WHERE id = ?1"), [id.0])
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let mut item = item_from(&row)?;
    drop(rows);
    complete(connection, &mut item).await?;
    Ok(Some(item))
}

pub(crate) async fn required(connection: &Connection, id: ItemId) -> Result<Item, Error> {
    fetch(connection, id)
        .await?
        .ok_or_else(|| Refusal::NotFound { item: id }.into())
}

/// Adds the item's waits and images.
async fn complete(connection: &Connection, item: &mut Item) -> Result<(), Error> {
    item.waits = waits_of(connection, item.id).await?;
    item.images = crate::images::images_of(connection, item.id).await?;
    Ok(())
}

async fn waits_of(connection: &Connection, id: ItemId) -> Result<Vec<Wait>, Error> {
    let mut rows = connection
        .query(
            "SELECT i.id, i.title, i.status, i.summary FROM tasks_waits w
                JOIN tasks_items i ON i.id = w.waits_on
             WHERE w.item = ?1 ORDER BY i.id",
            [id.0],
        )
        .await?;
    let mut waits = Vec::new();
    while let Some(row) = rows.next().await? {
        waits.push(Wait {
            item: ItemId(row.get(0)?),
            title: row.get(1)?,
            status: status_of(&row.get::<String>(2)?)?,
            summary: row.get(3)?,
        });
    }
    Ok(waits)
}

fn item_from(row: &Row) -> Result<Item, Error> {
    let kind = row.get::<String>(4)?;
    let priority = row.get::<i64>(5)?;
    Ok(Item {
        id: ItemId(row.get(0)?),
        project: row.get(1)?,
        title: row.get(2)?,
        notes: row.get(3)?,
        kind: Kind::parse(&kind)
            .ok_or_else(|| invalid_row("tasks_items", format!("kind {kind:?}")))?,
        priority: Priority::new(priority)
            .ok_or_else(|| invalid_row("tasks_items", format!("priority {priority}")))?,
        status: status_of(&row.get::<String>(6)?)?,
        is_blocker_report: row.get::<i64>(7)? == 1,
        why: row.get(8)?,
        filed_by: session_of(row.get(9)?)?,
        filed_from: row.get(10)?,
        doing_session: session_of(row.get(11)?)?,
        summary: row.get(12)?,
        is_queued: row.get::<i64>(13)? == 1,
        waits: Vec::new(),
        images: Vec::new(),
        intent_revision: row.get(14)?,
        state_revision: row.get(15)?,
        created_at: row.get(16)?,
        changed_at: row.get(17)?,
        finished_by: session_of(row.get(18)?)?,
    })
}

fn status_of(text: &str) -> Result<Status, Error> {
    Status::parse(text).ok_or_else(|| invalid_row("tasks_items", format!("status {text:?}")))
}

fn session_of(text: Option<String>) -> Result<Option<SessionId>, Error> {
    text.map(|text| {
        SessionId::new(text).map_err(|error| invalid_row("tasks_items", error.to_string()))
    })
    .transpose()
}

pub(crate) fn invalid_row(table: &'static str, reason: String) -> Error {
    Error::InvalidStoredRow { table, reason }
}
