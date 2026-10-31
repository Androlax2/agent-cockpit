//! Reading Turso's change capture (CDC v2, table `turso_cdc`) as committed transactions.
//!
//! Each captured row carries `change_txn_id`; each committed transaction ends with a COMMIT
//! row (`change_type = 2`) whose `change_id` is the transaction's revision. A rolled-back
//! transaction leaves no rows, so every row a reader sees belongs to a committed transaction.

use crate::{Error, ReadTransaction};

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

impl ReadTransaction {
    /// The revision of the last commit this transaction's snapshot sees; 0 before any commit.
    pub async fn revision(&self) -> Result<i64, Error> {
        let mut rows = self
            .query(
                "SELECT coalesce(max(change_id), 0) FROM turso_cdc WHERE change_type = ?1",
                [CHANGE_TYPE_COMMIT],
            )
            .await?;
        let row = rows
            .next()
            .await?
            .ok_or(turso::Error::QueryReturnedNoRows)?;
        Ok(row.get(0)?)
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
            pending_changes.push(Change {
                kind,
                table: row.get(2)?,
                row_id: row.get(3)?,
            });
        }
        if commits.len() < max_commits && !pending_changes.is_empty() {
            return Err(Error::UnexpectedChange {
                change_id: revision,
                reason: "changes after the last COMMIT record".to_string(),
            });
        }
        Ok(commits)
    }
}
