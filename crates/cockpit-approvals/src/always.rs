//! "Always allow" for a host command (#1387, part 3). When the owner approves a request
//! always, its project's later asks that match it exactly are approved as they are filed:
//! the same command line in the same directory, the same card (folders and hosts), the same
//! checkout, and every file it is bound to ([`RunRequest::bound_files`]) with the hash it had
//! when the owner approved. A changed file or a different argument asks again; there are no
//! patterns. A request whose files cannot all be told is never approved always, since a
//! change to a file nobody pinned would not ask again.
//!
//! An "always" approval stays until the owner revokes it, or approves the same command line
//! always again after a file changed, which replaces it.

use std::path::PathBuf;

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::error::invalid;
use crate::rows::{Project, invalid_row, next_id, not_found, stored_row};
use crate::runs::{approved_of, lines_column, lines_from, stored_run, text_of, writes_column};
use crate::sudo_editable::{RootFile, decode_pinned, encode_pinned};
use crate::{Approved, DecidedThrough, Error, Reach, Refusal, RunId, RunRequest, RunState};

const TABLE: &str = "approvals_always";
const KIND: &str = "always approval";

/// The columns `always_from` reads, in its order.
const ALWAYS_COLUMNS: &str = "id, project, project_root, cwd, command, card_writes, card_hosts,
        files, request, granted_at, granted_through, revoked_at, revoked_through";

/// One "always" approval of a project's command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlwaysApproval {
    pub id: i64,
    /// The project whose sessions' asks it approves, and the checkout they ask from.
    pub project: Project,
    pub cwd: PathBuf,
    pub command: String,
    pub reach: Reach,
    /// The files it is bound to, each with the hash it had when the owner approved it.
    pub files: Vec<RootFile>,
    /// The request whose approval made it.
    pub request: RunId,
    pub granted: OwnerAct,
    pub revoked: Option<OwnerAct>,
}

/// When the owner made or revoked an "always" approval, and through which client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerAct {
    pub at: i64,
    pub through: Option<DecidedThrough>,
}

/// Why `run` cannot be approved always, or `None` when it can.
fn not_always(run: &RunRequest) -> Option<&'static str> {
    if run.start.is_some() {
        return Some("a request to start a session is decided each time");
    }
    if run.project.is_none() {
        return Some("it was not asked from a registered project's checkout");
    }
    let (Some(root_runs), Some(findings)) = (&run.root_runs, &run.findings) else {
        return Some("the files it runs were not checked");
    };
    if run.pinned.is_none()
        || root_runs.cannot_tell
        || findings.iter().any(crate::rule_scan::says_cannot_tell)
    {
        return Some(
            "Torlek cannot tell every file it runs, so a changed file would not ask again",
        );
    }
    if findings.iter().any(crate::rule_scan::runs_unhashed_code) {
        return Some("it runs code it downloads or decodes, which no hash binds");
    }
    None
}

/// Makes the owner's approval of `run`, just approved in this transaction, an "always"
/// approval for its project. Refused, with nothing written, when it cannot be narrowed to
/// its exact command and files; the caller's transaction then approves nothing either. An
/// active one for the same command line, directory, card and checkout is revoked: the new
/// one names the files as the owner read them now.
pub async fn approve_always(
    write: &WriteTransaction<'_>,
    run: &RunRequest,
    through: Option<DecidedThrough>,
    now: i64,
) -> Result<AlwaysApproval, Error> {
    if run.state != RunState::Approved {
        return Err(invalid("request", "is not approved").into());
    }
    if let Some(why) = not_always(run) {
        return Err(Refusal::NotAlways {
            why: why.to_owned(),
        }
        .into());
    }
    let project = run
        .project
        .as_ref()
        .expect("not_always refuses a request without a project");
    let project_root = text_of(&project.root)?;
    let digest = run.digest();
    let through = through.map(DecidedThrough::as_str);
    write
        .execute(
            "UPDATE approvals_always SET revoked_at = ?4, revoked_through = ?5
             WHERE project = ?1 AND project_root = ?2 AND digest = ?3 AND revoked_at IS NULL",
            (
                project.name.as_str(),
                project_root,
                digest.as_str(),
                now,
                through,
            ),
        )
        .await?;
    let id = next_id(write, TABLE).await?;
    stored_row(
        write,
        &format!(
            "INSERT INTO approvals_always (id, project, project_root, cwd, command, card_writes,
                    card_hosts, files, digest, request, granted_at, granted_through)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 RETURNING {ALWAYS_COLUMNS}"
        ),
        (
            id,
            project.name.as_str(),
            project_root,
            text_of(&run.cwd)?,
            run.command.as_str(),
            writes_column(&run.reach)?,
            lines_column(&run.reach.hosts),
            encode_pinned(&run.bound_files()),
            digest.as_str(),
            run.id.0,
            now,
            through,
        ),
        always_from,
    )
    .await
}

/// Approves `run`, just filed and checked in this transaction, when one of its project's
/// "always" approvals names it exactly; `None` leaves it asking the owner. Returns the run
/// to carry out and the request as it now reads, its decision naming the "always" approval.
pub async fn approve_by_always(
    write: &WriteTransaction<'_>,
    run: &RunRequest,
    now: i64,
) -> Result<Option<(Approved, RunRequest)>, Error> {
    if run.state != RunState::Asking || not_always(run).is_some() {
        return Ok(None);
    }
    let project = run
        .project
        .as_ref()
        .expect("not_always refuses a request without a project");
    let digest = run.digest();
    let mut rows = write
        .query(
            "SELECT id FROM approvals_always
             WHERE project = ?1 AND project_root = ?2 AND digest = ?3 AND files = ?4
                AND revoked_at IS NULL
             ORDER BY id LIMIT 1",
            (
                project.name.as_str(),
                text_of(&project.root)?,
                digest.as_str(),
                encode_pinned(&run.bound_files()),
            ),
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let always: i64 = row.get(0)?;
    drop(rows);
    let run = stored_run(
        write,
        "UPDATE approvals_runs SET state = 'approved', decided_at = ?2, approved_digest = ?3,
                approved_always = ?4
             WHERE id = ?1 AND state = 'asking'",
        (run.id.0, now, digest.as_str(), always),
        now,
    )
    .await?;
    Ok(Some((approved_of(&run), run)))
}

/// The "always" approvals that still approve, oldest first.
pub async fn always_approvals(connection: &Connection) -> Result<Vec<AlwaysApproval>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {ALWAYS_COLUMNS} FROM approvals_always WHERE revoked_at IS NULL
                 ORDER BY id"
            ),
            (),
        )
        .await?;
    let mut approvals = Vec::new();
    while let Some(row) = rows.next().await? {
        approvals.push(always_from(&row)?);
    }
    Ok(approvals)
}

/// The owner revokes "always" approval `id`: its command line asks again from the next ask.
pub async fn revoke_always(
    write: &WriteTransaction<'_>,
    id: i64,
    through: Option<DecidedThrough>,
    now: i64,
) -> Result<AlwaysApproval, Error> {
    let mut rows = write
        .query(
            &format!("SELECT {ALWAYS_COLUMNS} FROM approvals_always WHERE id = ?1"),
            [id],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Err(not_found(KIND, id).into());
    };
    let current = always_from(&row)?;
    drop(rows);
    if current.revoked.is_some() {
        return Err(Refusal::AlwaysRevoked.into());
    }
    stored_row(
        write,
        &format!(
            "UPDATE approvals_always SET revoked_at = ?2, revoked_through = ?3 WHERE id = ?1
             RETURNING {ALWAYS_COLUMNS}"
        ),
        (id, now, through.map(DecidedThrough::as_str)),
        always_from,
    )
    .await
}

fn always_from(row: &Row) -> Result<AlwaysApproval, Error> {
    let files: String = row.get(7)?;
    Ok(AlwaysApproval {
        id: row.get(0)?,
        project: Project {
            name: row.get(1)?,
            root: PathBuf::from(row.get::<String>(2)?),
        },
        cwd: PathBuf::from(row.get::<String>(3)?),
        command: row.get(4)?,
        reach: Reach {
            writes: lines_from(row, 5)?.into_iter().map(PathBuf::from).collect(),
            hosts: lines_from(row, 6)?,
        },
        files: decode_pinned(&files)
            .ok_or_else(|| invalid_row(TABLE, format!("files {files:?}")))?,
        request: RunId(row.get(8)?),
        granted: OwnerAct {
            at: row.get(9)?,
            through: through_from(row, 10)?,
        },
        revoked: match row.get::<Option<i64>>(11)? {
            Some(at) => Some(OwnerAct {
                at,
                through: through_from(row, 12)?,
            }),
            None => None,
        },
    })
}

fn through_from(row: &Row, column: usize) -> Result<Option<DecidedThrough>, Error> {
    match row.get::<Option<String>>(column)? {
        Some(text) => Ok(Some(DecidedThrough::parse(&text).ok_or_else(|| {
            invalid_row(TABLE, format!("decided through {text:?}"))
        })?)),
        None => Ok(None),
    }
}
