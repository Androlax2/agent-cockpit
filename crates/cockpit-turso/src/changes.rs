//! Reading Turso's change capture (CDC v2, table `turso_cdc`) as committed transactions.
//!
//! Each captured row carries `change_txn_id`; each committed transaction ends with a COMMIT
//! row (`change_type = 2`) whose `change_id` is the transaction's revision. A rolled-back
//! transaction leaves no rows, so every row a reader sees belongs to a committed transaction.

use std::num::NonZeroU64;

use crate::{Error, ReadTransaction, WriteTransaction};

const CHANGE_TYPE_DELETE: i64 = -1;
const CHANGE_TYPE_UPDATE: i64 = 0;
const CHANGE_TYPE_INSERT: i64 = 1;
const CHANGE_TYPE_COMMIT: i64 = 2;

/// One committed transaction's row changes, in the order they were made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    /// The global commit revision: increases with every commit of this database epoch.
    pub revision: i64,
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    /// `sqlite_schema` for schema changes.
    pub table: String,
    pub row_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Insert,
    Update,
    Delete,
}

impl ReadTransaction<'_> {
    /// The revision of the last commit this transaction's snapshot sees; 0 before any commit.
    pub async fn revision(&self) -> Result<i64, Error> {
        // Every visible transaction ends with its COMMIT row, so the newest row is one; reading
        // it by rowid avoids scanning the whole history for the newest COMMIT.
        let mut rows = self
            .query(
                "SELECT change_id, change_type FROM turso_cdc ORDER BY change_id DESC LIMIT 1",
                (),
            )
            .await?;
        let Some(row) = rows.next().await? else {
            return Ok(0);
        };
        let change_id: i64 = row.get(0)?;
        if row.get::<i64>(1)? != CHANGE_TYPE_COMMIT {
            return Err(Error::UnexpectedChange {
                change_id,
                reason: "the newest change is not a COMMIT record".to_string(),
            });
        }
        Ok(change_id)
    }

    /// Up to `max_commits` whole commits with a revision after `revision`, oldest first.
    /// Pass the last returned revision to continue.
    pub async fn commits_after(
        &self,
        revision: i64,
        max_commits: usize,
    ) -> Result<Vec<Commit>, Error> {
        let mut rows = self
            .query(
                "SELECT change_id, change_type, table_name, id FROM turso_cdc
                 WHERE change_id > ?1 ORDER BY change_id",
                [revision],
            )
            .await?;
        let mut commits = Vec::new();
        let mut pending_changes = Vec::new();
        let mut first_pending_change_id = None;
        while commits.len() < max_commits
            && let Some(row) = rows.next().await?
        {
            let change_id: i64 = row.get(0)?;
            let kind = match row.get::<i64>(1)? {
                CHANGE_TYPE_COMMIT => {
                    commits.push(Commit {
                        revision: change_id,
                        changes: std::mem::take(&mut pending_changes),
                    });
                    first_pending_change_id = None;
                    continue;
                }
                CHANGE_TYPE_INSERT => ChangeKind::Insert,
                CHANGE_TYPE_UPDATE => ChangeKind::Update,
                CHANGE_TYPE_DELETE => ChangeKind::Delete,
                other => {
                    return Err(Error::UnexpectedChange {
                        change_id,
                        reason: format!("unknown change type {other}"),
                    });
                }
            };
            first_pending_change_id.get_or_insert(change_id);
            pending_changes.push(Change {
                kind,
                table: row.get(2)?,
                row_id: row.get(3)?,
            });
        }
        if commits.len() < max_commits
            && let Some(change_id) = first_pending_change_id
        {
            return Err(Error::UnexpectedChange {
                change_id,
                reason: "changes after the last COMMIT record".to_string(),
            });
        }
        Ok(commits)
    }
}

impl WriteTransaction<'_> {
    /// Discards the captured changes of whole commits older than the newest `keep_changes`
    /// change rows, and returns the revision of the newest commit discarded, or `None` when no
    /// commit is that old. A reader that resumes after an older revision would miss changes,
    /// so the change feed must refuse it from then on.
    ///
    /// The newest commit is always kept: Turso numbers the next change after the newest row,
    /// so discarding it would let revisions repeat.
    pub async fn discard_changes(&self, keep_changes: NonZeroU64) -> Result<Option<i64>, Error> {
        let Some(newest) = self
            .optional_integer("SELECT max(change_id) FROM turso_cdc", ())
            .await?
        else {
            return Ok(None);
        };
        let keep = i64::try_from(keep_changes.get()).unwrap_or(i64::MAX);
        let Some(discarded_through) = self
            .optional_integer(
                "SELECT max(change_id) FROM turso_cdc WHERE change_type = ?1 AND change_id <= ?2",
                (CHANGE_TYPE_COMMIT, newest.saturating_sub(keep)),
            )
            .await?
        else {
            return Ok(None);
        };
        self.execute(
            "DELETE FROM turso_cdc WHERE change_id <= ?1",
            [discarded_through],
        )
        .await?;
        Ok(Some(discarded_through))
    }

    /// The first column of the first row, which an aggregate makes `NULL` over no rows.
    async fn optional_integer(
        &self,
        sql: &str,
        params: impl turso::IntoParams,
    ) -> Result<Option<i64>, Error> {
        let mut rows = self.query(sql, params).await?;
        match rows.next().await? {
            Some(row) => Ok(row.get::<Option<i64>>(0)?),
            None => Ok(None),
        }
    }
}
