//! Published entities: the state producers put on the change feed, one row per entity.
//!
//! A producer publishes an entity's complete current state in the same transaction as the
//! change that caused it, and retracts it when the entity is gone. The row keeps its
//! tombstone, so the entity's revision never goes back when the same id is published again.

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::Connection;
use serde_json::{Map, Value};

use crate::error::{Error, invalid, invalid_row, stored_revision};

pub(crate) const TABLE: &str = "messaging_entities";
const LONGEST_KIND: usize = 64;
const LONGEST_ENTITY_ID: usize = 256;

/// What [`publish`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Published {
    /// The state differs from the one published before; the entity is at `revision` now.
    Changed { revision: u64 },
    /// The same state was published already: nothing is written, so nothing reaches the
    /// feed.
    Unchanged { revision: u64 },
}

/// Puts `state` on the change feed as the current state of the entity `id` of `kind`.
///
/// `kind` is lower-case kebab case (`session`, `task`); the producer owns what kinds and
/// states mean. Commit it with the change that caused it.
pub async fn publish(
    write: &WriteTransaction<'_>,
    kind: &str,
    id: &str,
    state: &Map<String, Value>,
) -> Result<Published, Error> {
    validate(kind, id)?;
    let current = stored(write, kind, id).await?;
    if let Some(Stored {
        revision,
        state: Some(published),
    }) = &current
        && published == state
    {
        return Ok(Published::Unchanged {
            revision: *revision,
        });
    }
    let text = Value::Object(state.clone()).to_string();
    let revision = match current {
        Some(Stored { revision, .. }) => {
            write
                .execute(
                    "UPDATE messaging_entities SET revision = ?3, state = ?4
                     WHERE kind = ?1 AND entity = ?2",
                    (kind, id, signed(revision + 1), text),
                )
                .await?;
            revision + 1
        }
        None => {
            write
                .execute(
                    "INSERT INTO messaging_entities (kind, entity, revision, state)
                     VALUES (?1, ?2, 1, ?3)",
                    (kind, id, text),
                )
                .await?;
            1
        }
    };
    Ok(Published::Changed { revision })
}

/// Takes the entity off the feed: subscribers receive its tombstone. Returns the tombstone's
/// revision, or `None` when the entity is not published.
pub async fn retract(
    write: &WriteTransaction<'_>,
    kind: &str,
    id: &str,
) -> Result<Option<u64>, Error> {
    validate(kind, id)?;
    let Some(Stored {
        revision,
        state: Some(_),
    }) = stored(write, kind, id).await?
    else {
        return Ok(None);
    };
    write
        .execute(
            "UPDATE messaging_entities SET revision = ?3, state = NULL
             WHERE kind = ?1 AND entity = ?2",
            (kind, id, signed(revision + 1)),
        )
        .await?;
    Ok(Some(revision + 1))
}

/// The ids of the entities of `kind` on the feed now, tombstones left out, ordered by id: a
/// producer that publishes from its own state retracts the ones its state no longer has.
pub async fn published_ids(connection: &Connection, kind: &str) -> Result<Vec<String>, Error> {
    validate_kind(kind)?;
    let mut rows = connection
        .query(
            "SELECT entity FROM messaging_entities
             WHERE kind = ?1 AND state IS NOT NULL ORDER BY entity",
            [kind],
        )
        .await?;
    let mut ids = Vec::new();
    while let Some(row) = rows.next().await? {
        ids.push(row.get(0)?);
    }
    Ok(ids)
}

/// One entity as the feed hands it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityChange {
    pub kind: String,
    pub id: String,
    pub revision: u64,
    /// `None`: the entity no longer exists.
    pub state: Option<Map<String, Value>>,
}

/// Every published entity, ordered by kind and id.
pub(crate) async fn all_published(connection: &Connection) -> Result<Vec<EntityChange>, Error> {
    let mut rows = connection
        .query(
            "SELECT kind, entity, revision, state FROM messaging_entities
             WHERE state IS NOT NULL ORDER BY kind, entity",
            (),
        )
        .await?;
    let mut entities = Vec::new();
    while let Some(row) = rows.next().await? {
        entities.push(entity_change(&row)?);
    }
    Ok(entities)
}

/// The entity stored in row `row_id`, tombstone included; `None` when no such row exists.
pub(crate) async fn by_row(
    connection: &Connection,
    row_id: i64,
) -> Result<Option<EntityChange>, Error> {
    let mut rows = connection
        .query(
            "SELECT kind, entity, revision, state FROM messaging_entities WHERE id = ?1",
            [row_id],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(entity_change(&row)?)),
        None => Ok(None),
    }
}

fn entity_change(row: &cockpit_turso::sql::Row) -> Result<EntityChange, Error> {
    let state = row
        .get::<Option<String>>(3)?
        .map(|text| parse_state(&text))
        .transpose()?;
    Ok(EntityChange {
        kind: row.get(0)?,
        id: row.get(1)?,
        revision: stored_revision(TABLE, row.get(2)?)?,
        state,
    })
}

struct Stored {
    revision: u64,
    state: Option<Map<String, Value>>,
}

async fn stored(connection: &Connection, kind: &str, id: &str) -> Result<Option<Stored>, Error> {
    let mut rows = connection
        .query(
            "SELECT revision, state FROM messaging_entities WHERE kind = ?1 AND entity = ?2",
            (kind, id),
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    Ok(Some(Stored {
        revision: stored_revision(TABLE, row.get(0)?)?,
        state: row
            .get::<Option<String>>(1)?
            .map(|text| parse_state(&text))
            .transpose()?,
    }))
}

fn parse_state(text: &str) -> Result<Map<String, Value>, Error> {
    match serde_json::from_str(text) {
        Ok(Value::Object(state)) => Ok(state),
        Ok(_) => Err(invalid_row(TABLE, "a published state is not a JSON object")),
        Err(error) => Err(invalid_row(
            TABLE,
            format!("a published state is not JSON: {error}"),
        )),
    }
}

fn validate(kind: &str, id: &str) -> Result<(), Error> {
    validate_kind(kind)?;
    if id.is_empty() || id.len() > LONGEST_ENTITY_ID {
        return Err(invalid(
            "id",
            format!("an entity id has 1 to {LONGEST_ENTITY_ID} bytes"),
        ));
    }
    Ok(())
}

fn validate_kind(kind: &str) -> Result<(), Error> {
    let is_kebab_case = kind.starts_with(|first: char| first.is_ascii_lowercase())
        && kind.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        });
    if !is_kebab_case || kind.len() > LONGEST_KIND {
        return Err(invalid(
            "kind",
            format!("{kind:?} is not lower-case kebab case of at most {LONGEST_KIND} bytes"),
        ));
    }
    Ok(())
}

/// Entity revisions are stored as SQLite integers; one past `i64::MAX` is out of reach.
fn signed(revision: u64) -> i64 {
    i64::try_from(revision).expect("an entity revision fits in an i64")
}
