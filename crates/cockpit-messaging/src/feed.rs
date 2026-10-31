//! The change feed over published entities: a snapshot and its revision from one read
//! transaction, then the changes after a cursor, then each new commit (report 04).
//!
//! Revisions are Turso's global commit revisions. A batch carries each changed entity once,
//! in its state at the batch's cursor, so a client that applies the whole batch holds exactly
//! the state of that revision. Commits that change no published entity produce no batch.

use std::collections::HashSet;
use std::num::NonZeroU64;

use cockpit_turso::sql::Connection;
use cockpit_turso::{Database, ReadTransaction, WriteTransaction};

use crate::entities::{self, EntityChange};
use crate::error::{Error, Refusal, ResnapshotReason, invalid_row};

/// A position in the feed: every change up to `revision` of the database `epoch` is applied.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FeedCursor {
    pub epoch: String,
    pub revision: u64,
}

/// Where a feed starts, and what a batch builds on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedPosition {
    /// Before everything: the batch is the complete current state.
    Snapshot,
    After(FeedCursor),
}

/// Changes ending at `cursor`, applied whole before the cursor is stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedBatch {
    pub follows: FeedPosition,
    pub cursor: FeedCursor,
    pub changes: Vec<EntityChange>,
}

/// One subscriber's place in the feed.
#[derive(Debug)]
pub struct Feed {
    /// The cursor of the last batch handed out, which the next one follows.
    sent: FeedCursor,
    /// The revision read up to; commits after `sent` and up to here changed no entity.
    scanned: i64,
}

impl Feed {
    /// Starts following the feed at `from`. The first batch is the snapshot, or the changes
    /// after the cursor when there are any. A cursor that cannot be resumed is refused with
    /// [`Refusal::ResnapshotRequired`], never answered with a silent jump.
    pub async fn open(
        database: &Database,
        from: FeedPosition,
    ) -> Result<(Feed, Option<FeedBatch>), Error> {
        let read = database.read().await?;
        let newest = read.revision().await?;
        let cursor_at = |revision| FeedCursor {
            epoch: database.epoch().to_owned(),
            revision,
        };
        let resumed = match from {
            FeedPosition::Snapshot => {
                let cursor = cursor_at(revision(newest)?);
                let snapshot = FeedBatch {
                    follows: FeedPosition::Snapshot,
                    cursor: cursor.clone(),
                    changes: entities::all_published(&read).await?,
                };
                let feed = Feed {
                    sent: cursor,
                    scanned: newest,
                };
                return Ok((feed, Some(snapshot)));
            }
            FeedPosition::After(cursor) => cursor,
        };
        if resumed.epoch != database.epoch() {
            return Err(resnapshot(ResnapshotReason::OtherEpoch));
        }
        let resumed_revision = i64::try_from(resumed.revision)
            .ok()
            .filter(|resumed_revision| *resumed_revision <= newest)
            .ok_or_else(|| {
                resnapshot(ResnapshotReason::Ahead {
                    newest: newest.unsigned_abs(),
                })
            })?;
        let mut feed = Feed {
            sent: resumed,
            scanned: resumed_revision,
        };
        let batch = feed.read_after_scanned(&read, newest).await?;
        Ok((feed, batch))
    }

    /// The cursor the next batch follows.
    pub fn cursor(&self) -> &FeedCursor {
        &self.sent
    }

    /// The changes committed since the last call, or `None` when no published entity
    /// changed. Refused with [`Refusal::ResnapshotRequired`] once the history it needs is
    /// discarded; the subscriber then starts again from a snapshot.
    pub async fn next(&mut self, database: &Database) -> Result<Option<FeedBatch>, Error> {
        let read = database.read().await?;
        let newest = read.revision().await?;
        self.read_after_scanned(&read, newest).await
    }

    async fn read_after_scanned(
        &mut self,
        read: &ReadTransaction<'_>,
        newest: i64,
    ) -> Result<Option<FeedBatch>, Error> {
        if newest < self.scanned {
            return Err(invalid_row(
                "turso_cdc",
                format!(
                    "the newest revision {newest} is older than {} read before",
                    self.scanned
                ),
            ));
        }
        if newest == self.scanned {
            return Ok(None);
        }
        let retained_after = retained_after(read).await?;
        if self.scanned < retained_after {
            return Err(resnapshot(ResnapshotReason::Discarded {
                retained_after: retained_after.unsigned_abs(),
            }));
        }
        let changes = changed_entities(read, self.scanned).await?;
        self.scanned = newest;
        if changes.is_empty() {
            return Ok(None);
        }
        let cursor = FeedCursor {
            epoch: self.sent.epoch.clone(),
            revision: revision(newest)?,
        };
        let follows = FeedPosition::After(std::mem::replace(&mut self.sent, cursor.clone()));
        Ok(Some(FeedBatch {
            follows,
            cursor,
            changes,
        }))
    }
}

/// Discards the change history older than the newest `keep_changes` captured rows, and
/// returns the revision the feed now resumes after at the earliest. Cursors before it are
/// answered `resnapshot-required`.
pub async fn discard_history(
    write: &WriteTransaction<'_>,
    keep_changes: NonZeroU64,
) -> Result<Option<u64>, Error> {
    let Some(discarded_through) = write.discard_changes(keep_changes).await? else {
        return Ok(None);
    };
    write
        .execute(
            "INSERT INTO messaging_feed_retention (id, retained_after) VALUES (1, ?1)
             ON CONFLICT (id) DO UPDATE SET retained_after = excluded.retained_after",
            [discarded_through],
        )
        .await?;
    Ok(Some(revision(discarded_through)?))
}

/// Each published entity changed by a commit after `after`, once, in its current state.
async fn changed_entities(
    read: &ReadTransaction<'_>,
    after: i64,
) -> Result<Vec<EntityChange>, Error> {
    let mut changed_rows = Vec::new();
    let mut seen_rows = HashSet::new();
    for commit in read.commits_after(after, usize::MAX).await? {
        for change in commit.changes {
            if change.table == entities::TABLE && seen_rows.insert(change.row_id) {
                changed_rows.push(change.row_id);
            }
        }
    }
    let mut changes = Vec::with_capacity(changed_rows.len());
    for row_id in changed_rows {
        // Entity rows are never deleted, tombstones included.
        let entity = entities::by_row(read, row_id).await?.ok_or_else(|| {
            invalid_row(entities::TABLE, format!("the changed row {row_id} is gone"))
        })?;
        changes.push(entity);
    }
    Ok(changes)
}

async fn retained_after(connection: &Connection) -> Result<i64, Error> {
    let mut rows = connection
        .query(
            "SELECT retained_after FROM messaging_feed_retention WHERE id = 1",
            (),
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(row.get(0)?),
        None => Ok(0),
    }
}

fn revision(revision: i64) -> Result<u64, Error> {
    u64::try_from(revision)
        .map_err(|_| invalid_row("turso_cdc", format!("negative revision {revision}")))
}

fn resnapshot(reason: ResnapshotReason) -> Error {
    Error::Refused(Refusal::ResnapshotRequired(reason))
}
