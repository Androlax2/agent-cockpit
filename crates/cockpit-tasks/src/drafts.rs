//! Form text the owner has not saved yet, kept in the state database so a client reload or a
//! daemon restart does not lose it. One draft per new-item form of a project and one per
//! item; saving replaces it. An item's draft remembers the intent revision it started from,
//! so the client can tell the owner the item changed underneath it.

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::items::{invalid_row, owner_only, required};
use crate::text::{self, NOTES_MAX_CHARS, TITLE_MAX_CHARS};
use crate::{Actor, Draft, DraftTarget, Error, ItemId};

/// Saves the form text for `target`, replacing its draft. For an item,
/// `base_intent_revision` is the intent revision the form was opened on; without it, the
/// item's current one.
pub async fn save_draft(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    target: &DraftTarget,
    title: &str,
    notes: &str,
    base_intent_revision: Option<i64>,
    now: i64,
) -> Result<Draft, Error> {
    owner_only(actor, "keep a draft")?;
    // A draft may be unfinished: blank or several lines are fine; only the limits apply.
    text::body("title", title, TITLE_MAX_CHARS)?;
    text::body("notes", notes, NOTES_MAX_CHARS)?;
    let (project, item, base) = match target {
        DraftTarget::New { project } => {
            if project.is_empty() {
                return Err(crate::error::invalid("project", "is empty").into());
            }
            (Some(project.as_str()), None, None)
        }
        DraftTarget::Item(id) => {
            let current = required(write, *id).await?.intent_revision;
            let base = match base_intent_revision {
                Some(base) if base < 1 || base > current => {
                    return Err(crate::error::invalid(
                        "base revision",
                        format!("must be between 1 and the item's {current}"),
                    )
                    .into());
                }
                Some(base) => base,
                None => current,
            };
            (None, Some(id.0), Some(base))
        }
    };
    discard_draft(write, actor, target).await?;
    write
        .execute(
            "INSERT INTO tasks_drafts (project, item, base_intent_revision, title, notes,
                saved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (project, item, base, title, notes, now),
        )
        .await?;
    Ok(drafts(write, actor)
        .await?
        .into_iter()
        .find(|draft| draft.target == *target)
        .expect("the draft was saved in this transaction"))
}

/// Every draft, oldest save first.
pub async fn drafts(connection: &Connection, actor: &Actor) -> Result<Vec<Draft>, Error> {
    owner_only(actor, "read drafts")?;
    let mut rows = connection
        .query(
            "SELECT d.project, d.item, d.base_intent_revision, d.title, d.notes, d.saved_at,
                i.intent_revision
             FROM tasks_drafts d LEFT JOIN tasks_items i ON i.id = d.item
             ORDER BY d.saved_at, d.id",
            (),
        )
        .await?;
    let mut drafts = Vec::new();
    while let Some(row) = rows.next().await? {
        drafts.push(draft_from(&row)?);
    }
    Ok(drafts)
}

/// Forgets the draft for `target`; none is not an error.
pub async fn discard_draft(
    write: &WriteTransaction<'_>,
    actor: &Actor,
    target: &DraftTarget,
) -> Result<(), Error> {
    owner_only(actor, "discard a draft")?;
    match target {
        DraftTarget::New { project } => {
            write
                .execute(
                    "DELETE FROM tasks_drafts WHERE project = ?1",
                    [project.as_str()],
                )
                .await?
        }
        DraftTarget::Item(id) => {
            write
                .execute("DELETE FROM tasks_drafts WHERE item = ?1", [id.0])
                .await?
        }
    };
    Ok(())
}

fn draft_from(row: &Row) -> Result<Draft, Error> {
    let target = match (row.get::<Option<String>>(0)?, row.get::<Option<i64>>(1)?) {
        (Some(project), None) => DraftTarget::New { project },
        (None, Some(item)) => DraftTarget::Item(ItemId(item)),
        other => {
            return Err(invalid_row(
                "tasks_drafts",
                format!("target {other:?} is not one project or one item"),
            ));
        }
    };
    let base_intent_revision = row.get::<Option<i64>>(2)?;
    let current = row.get::<Option<i64>>(6)?;
    Ok(Draft {
        target,
        title: row.get(3)?,
        notes: row.get(4)?,
        is_stale: base_intent_revision.is_some() && base_intent_revision != current,
        base_intent_revision,
        saved_at: row.get(5)?,
    })
}
