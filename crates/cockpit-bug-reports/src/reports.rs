//! Filing, reading and the owner's decisions on reports, inside the caller's transaction.

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row, params};

use crate::error::{invalid, invalid_row};
use crate::text::{self, DESCRIPTION_MAX_CHARS, MERGED_DESCRIPTION_MAX_CHARS, STEPS_MAX_CHARS};
use crate::{
    Bundle, Edit, Error, Facts, Filed, Fingerprint, Kind, NewReport, Origin, Refusal, Report,
    ReportId, Source, Status, UNKNOWN_COMPONENT,
};

/// The most drafts one session opens in a day, owner-requested ones included: the claim
/// that the owner asked lifts the per-fingerprint limit, never this one, so a session in a
/// loop cannot flood the owner whatever it claims.
pub const SESSION_DRAFTS_PER_DAY: u32 = 5;

const SECONDS_PER_DAY: i64 = 24 * 60 * 60;

const COLUMNS: &str = "id, origin, requested_by_owner, session, status, title, description, \
    steps, version, kind, component, environment, bundle_path, bundle_contents, bundle_problem, \
    occurrences, issue_url, created_at, changed_at";

/// Files `new`. A limited source (an agent's own report, the daemon) opens at most one draft
/// per fingerprint per UTC day: a repeat counts as one more occurrence of today's draft, or
/// opens nothing once that one was sent or discarded. The owner's reports and the ones a
/// session says the owner asked for always open a new draft.
pub async fn file(
    write: &WriteTransaction<'_>,
    facts: &Facts,
    new: NewReport,
    now: i64,
) -> Result<Filed, Error> {
    let description = text::required_body("description", &new.description, DESCRIPTION_MAX_CHARS)?;
    let steps = text::body("steps", &new.steps, STEPS_MAX_CHARS)?;
    let title = match &new.title {
        Some(title) => text::title(title)?,
        None => text::derived_title(&description),
    };
    let component = match &new.component {
        Some(component) => text::component(component)?,
        None => UNKNOWN_COMPONENT.to_owned(),
    };
    if facts.version.trim().is_empty() {
        return Err(invalid("version", "is empty").into());
    }
    let session = match &new.source {
        Source::Agent { session, .. } if session.is_empty() => {
            return Err(invalid("session", "is empty").into());
        }
        Source::Agent { session, .. } => Some(session.as_str()),
        _ => None,
    };
    let fingerprint = Fingerprint {
        version: facts.version.clone(),
        kind: new.kind,
        component,
    };
    let day = now.div_euclid(SECONDS_PER_DAY);
    if new.source.is_limited()
        && let Some(today) = limited_today(write, &fingerprint, day).await?
    {
        return match today.status {
            Status::Draft => Ok(Filed::Merged(merge(write, today, &description, now).await?)),
            Status::Sent | Status::Discarded => Ok(Filed::AlreadyHandled(today)),
        };
    }
    if let Some(session) = session {
        check_session_limit(write, session, day).await?;
    }
    let requested_by_owner = matches!(
        new.source,
        Source::Agent {
            requested_by_owner: true,
            ..
        }
    );
    // AUTOINCREMENT: a discarded report's number is never given to another.
    let id = write
        .prepare(
            "INSERT INTO bug_reports (origin, requested_by_owner, session, is_limited, status,
                title, description, steps, version, kind, component, environment, occurrences,
                day, created_at, changed_at)
             VALUES (?1, ?2, ?3, ?4, 'draft', ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, ?12, ?13, ?13)
             RETURNING id",
        )
        .await?
        .query_row(params![
            new.source.origin().as_str(),
            i64::from(requested_by_owner),
            session,
            i64::from(new.source.is_limited()),
            title.as_str(),
            description.as_str(),
            steps.as_str(),
            fingerprint.version.as_str(),
            fingerprint.kind.as_str(),
            fingerprint.component.as_str(),
            facts.environment.as_str(),
            day,
            now,
        ])
        .await?
        .get::<i64>(0)?;
    Ok(Filed::Created(required(write, ReportId(id)).await?))
}

/// Records the support bundle written for a draft, or why none could be. Only a pending
/// bundle is recorded: a draft has one bundle, from the moment it was opened.
pub async fn attach_bundle(
    write: &WriteTransaction<'_>,
    id: ReportId,
    bundle: &Bundle,
    now: i64,
) -> Result<Report, Error> {
    let report = required(write, id).await?;
    if report.bundle != Bundle::Pending {
        return Err(invalid("bundle", "the report has its bundle already").into());
    }
    let (path, contents, problem) = match bundle {
        Bundle::Pending => return Err(invalid("bundle", "is pending").into()),
        Bundle::Written { path, contents } => (Some(path.as_str()), Some(contents.as_str()), None),
        Bundle::Failed { problem } => (None, None, Some(problem.as_str())),
    };
    write
        .execute(
            "UPDATE bug_reports SET bundle_path = ?2, bundle_contents = ?3,
                bundle_problem = ?4, changed_at = ?5
             WHERE id = ?1",
            params![id.0, path, contents, problem, now],
        )
        .await?;
    required(write, id).await
}

/// The drafts waiting for the owner, oldest first; `all` adds the sent and discarded ones.
pub async fn list(connection: &Connection, all: bool) -> Result<Vec<Report>, Error> {
    let sql =
        format!("SELECT {COLUMNS} FROM bug_reports WHERE ?1 = 1 OR status = 'draft' ORDER BY id");
    let mut rows = connection.query(&sql, [i64::from(all)]).await?;
    let mut reports = Vec::new();
    while let Some(row) = rows.next().await? {
        reports.push(report_from(&row)?);
    }
    Ok(reports)
}

pub async fn report(connection: &Connection, id: ReportId) -> Result<Report, Error> {
    required(connection, id).await
}

/// The owner's changes to a draft.
pub async fn edit(
    write: &WriteTransaction<'_>,
    id: ReportId,
    edit: &Edit,
    now: i64,
) -> Result<Report, Error> {
    let report = draft(write, id, "be edited").await?;
    if *edit == Edit::default() {
        return Err(invalid("edit", "names no field to change").into());
    }
    let title = match &edit.title {
        Some(title) => text::title(title)?,
        None => report.title,
    };
    let description = match &edit.description {
        Some(description) => {
            text::required_body("description", description, MERGED_DESCRIPTION_MAX_CHARS)?
        }
        None => report.description,
    };
    let steps = match &edit.steps {
        Some(steps) => text::body("steps", steps, STEPS_MAX_CHARS)?,
        None => report.steps,
    };
    write
        .execute(
            "UPDATE bug_reports SET title = ?2, description = ?3, steps = ?4, changed_at = ?5
             WHERE id = ?1",
            params![
                id.0,
                title.as_str(),
                description.as_str(),
                steps.as_str(),
                now
            ],
        )
        .await?;
    required(write, id).await
}

/// Records that the owner sent the draft; `issue_url` when the send returned the issue.
pub async fn mark_sent(
    write: &WriteTransaction<'_>,
    id: ReportId,
    issue_url: Option<&str>,
    now: i64,
) -> Result<Report, Error> {
    draft(write, id, "be sent").await?;
    if let Some(url) = issue_url
        && !url.starts_with("https://")
    {
        return Err(invalid("issue URL", "must start with https://").into());
    }
    write
        .execute(
            "UPDATE bug_reports SET status = 'sent', issue_url = ?2, changed_at = ?3
             WHERE id = ?1",
            params![id.0, issue_url, now],
        )
        .await?;
    required(write, id).await
}

/// The owner drops the draft. Its row stays, so the daily limit still counts it; the caller
/// removes its bundle file.
pub async fn discard(
    write: &WriteTransaction<'_>,
    id: ReportId,
    now: i64,
) -> Result<Report, Error> {
    draft(write, id, "be discarded").await?;
    write
        .execute(
            "UPDATE bug_reports SET status = 'discarded', changed_at = ?2 WHERE id = ?1",
            params![id.0, now],
        )
        .await?;
    required(write, id).await
}

/// How many drafts wait for the owner.
pub async fn drafts_waiting(connection: &Connection) -> Result<u64, Error> {
    let count = connection
        .query(
            "SELECT COUNT(*) FROM bug_reports WHERE status = 'draft'",
            (),
        )
        .await?
        .next()
        .await?
        .ok_or_else(|| invalid_row("COUNT(*) returned no row".to_owned()))?
        .get::<i64>(0)?;
    u64::try_from(count).map_err(|_| invalid_row(format!("negative count {count}")))
}

/// Today's report of a limited source with this fingerprint, the first one opened.
async fn limited_today(
    connection: &Connection,
    fingerprint: &Fingerprint,
    day: i64,
) -> Result<Option<Report>, Error> {
    let sql = format!(
        "SELECT {COLUMNS} FROM bug_reports
         WHERE is_limited = 1 AND version = ?1 AND kind = ?2 AND component = ?3 AND day = ?4
         ORDER BY id LIMIT 1"
    );
    let mut rows = connection
        .query(
            &sql,
            params![
                fingerprint.version.as_str(),
                fingerprint.kind.as_str(),
                fingerprint.component.as_str(),
                day
            ],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(report_from(&row)?)),
        None => Ok(None),
    }
}

/// One more occurrence of `today`. A description it does not hold yet is added while the
/// draft stays within its bound, so the owner reads what each repeat said.
async fn merge(
    write: &WriteTransaction<'_>,
    today: Report,
    description: &str,
    now: i64,
) -> Result<Report, Error> {
    let occurrence = today.occurrences + 1;
    let mut merged = today.description.clone();
    if !merged.contains(description) {
        let addition = format!("\n\nAlso reported (occurrence {occurrence}):\n{description}");
        if merged.chars().count() + addition.chars().count() <= MERGED_DESCRIPTION_MAX_CHARS {
            merged.push_str(&addition);
        }
    }
    write
        .execute(
            "UPDATE bug_reports SET occurrences = ?2, description = ?3, changed_at = ?4
             WHERE id = ?1",
            params![today.id.0, occurrence, merged.as_str(), now],
        )
        .await?;
    required(write, today.id).await
}

async fn check_session_limit(
    connection: &Connection,
    session: &str,
    day: i64,
) -> Result<(), Error> {
    let opened = connection
        .query(
            "SELECT COUNT(*) FROM bug_reports WHERE session = ?1 AND day = ?2",
            params![session, day],
        )
        .await?
        .next()
        .await?
        .ok_or_else(|| invalid_row("COUNT(*) returned no row".to_owned()))?
        .get::<i64>(0)?;
    if opened >= i64::from(SESSION_DRAFTS_PER_DAY) {
        return Err(Refusal::SessionLimit {
            limit: SESSION_DRAFTS_PER_DAY,
        }
        .into());
    }
    Ok(())
}

/// The report, refused unless it is still a draft.
async fn draft(
    connection: &Connection,
    id: ReportId,
    action: &'static str,
) -> Result<Report, Error> {
    let report = required(connection, id).await?;
    if report.status != Status::Draft {
        return Err(Refusal::WrongStatus {
            report: id,
            status: report.status,
            action,
        }
        .into());
    }
    Ok(report)
}

async fn required(connection: &Connection, id: ReportId) -> Result<Report, Error> {
    let sql = format!("SELECT {COLUMNS} FROM bug_reports WHERE id = ?1");
    let mut rows = connection.query(&sql, [id.0]).await?;
    match rows.next().await? {
        Some(row) => report_from(&row),
        None => Err(Refusal::NotFound { report: id }.into()),
    }
}

fn report_from(row: &Row) -> Result<Report, Error> {
    let kind = row.get::<String>(9)?;
    let bundle = match (
        row.get::<Option<String>>(12)?,
        row.get::<Option<String>>(13)?,
        row.get::<Option<String>>(14)?,
    ) {
        (None, None, None) => Bundle::Pending,
        (Some(path), Some(contents), None) => Bundle::Written { path, contents },
        (None, None, Some(problem)) => Bundle::Failed { problem },
        _ => return Err(invalid_row("bundle columns disagree".to_owned())),
    };
    Ok(Report {
        id: ReportId(row.get(0)?),
        origin: Origin::parse(&row.get::<String>(1)?)?,
        requested_by_owner: row.get::<i64>(2)? == 1,
        session: row.get(3)?,
        status: Status::parse(&row.get::<String>(4)?)?,
        title: row.get(5)?,
        description: row.get(6)?,
        steps: row.get(7)?,
        fingerprint: Fingerprint {
            version: row.get(8)?,
            kind: Kind::parse(&kind).ok_or_else(|| invalid_row(format!("kind {kind:?}")))?,
            component: row.get(10)?,
        },
        environment: row.get(11)?,
        bundle,
        occurrences: row.get(15)?,
        issue_url: row.get(16)?,
        created_at: row.get(17)?,
        changed_at: row.get(18)?,
    })
}
