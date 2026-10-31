//! The sessions tables and every query on them: current state, launches and the transition
//! history (report 04: current state + transition history, no event sourcing).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use cockpit_processes::ExecutionId;
use cockpit_projects::CheckoutId;
use cockpit_provider_api::{ConversationId, ProviderName};
use cockpit_turso::sql::{Connection, Row, Value};
use cockpit_turso::{Migration, MigrationSet, WriteTransaction};

use crate::{Error, Launch, LaunchKind, Session, SessionId, SessionState};

/// A session row is never deleted, so an id is never reused and an old reference reads as
/// ended. `stopping_to` is where a stop under way leads.
pub const MIGRATIONS: MigrationSet = MigrationSet {
    owner: "sessions",
    migrations: &[Migration {
        version: 1,
        sql: "CREATE TABLE sessions (
                id TEXT PRIMARY KEY CHECK (id <> ''),
                provider TEXT NOT NULL CHECK (provider <> ''),
                checkout_id INTEGER NOT NULL,
                holds_checkout INTEGER NOT NULL CHECK (holds_checkout IN (0, 1)),
                working_dir TEXT NOT NULL CHECK (working_dir <> ''),
                conversation TEXT CHECK (conversation <> ''),
                design_link TEXT CHECK (design_link <> ''),
                state TEXT NOT NULL CHECK (state IN
                    ('launching', 'running', 'stopping', 'asleep', 'ended', 'failed')),
                stopping_to TEXT CHECK (stopping_to IN ('asleep', 'ended', 'restart')),
                reason TEXT,
                revision INTEGER NOT NULL CHECK (revision >= 1),
                created_at INTEGER NOT NULL,
                changed_at INTEGER NOT NULL,
                CHECK ((state = 'stopping') = (stopping_to IS NOT NULL))
            );
            CREATE TABLE session_launches (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL,
                kind TEXT NOT NULL
                    CHECK (kind IN ('start', 'restart', 'wake', 'resume', 'restore')),
                execution_id INTEGER NOT NULL,
                launched_at INTEGER NOT NULL
            );
            CREATE INDEX session_launches_by_session ON session_launches (session_id, id);
            CREATE TABLE session_transitions (
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL,
                from_state TEXT,
                to_state TEXT NOT NULL,
                reason TEXT,
                at INTEGER NOT NULL
            );",
    }],
};

pub(crate) fn now() -> i64 {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970")
        .as_secs();
    i64::try_from(seconds).expect("seconds since 1970 fit in an i64")
}

pub(crate) fn path_text(path: &Path) -> Result<&str, Error> {
    path.to_str().ok_or_else(|| Error::InvalidStoredRow {
        reason: format!("{} is not valid UTF-8", path.display()),
    })
}

fn optional_text(text: Option<&str>) -> Value {
    text.map_or(Value::Null, |text| Value::Text(text.to_owned()))
}

const SESSION_COLUMNS: &str = "s.id, s.provider, s.checkout_id, s.holds_checkout, s.working_dir,
    s.conversation, s.design_link, s.state, s.stopping_to, s.reason, s.revision, s.created_at,
    s.changed_at, l.id, l.kind, l.execution_id, l.launched_at";
const SESSION_FROM: &str = "sessions s LEFT JOIN session_launches l ON l.id =
    (SELECT max(id) FROM session_launches WHERE session_id = s.id)";

async fn query_sessions(
    connection: &Connection,
    filter: &str,
    params: impl cockpit_turso::sql::IntoParams,
) -> Result<Vec<Session>, Error> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {SESSION_COLUMNS} FROM {SESSION_FROM} WHERE {filter}
                 ORDER BY s.created_at, s.id"
            ),
            params,
        )
        .await?;
    let mut sessions = Vec::new();
    while let Some(row) = rows.next().await? {
        sessions.push(session_from_row(&row)?);
    }
    Ok(sessions)
}

/// Every session, or only those not ended, by creation.
pub(crate) async fn sessions(
    connection: &Connection,
    include_ended: bool,
) -> Result<Vec<Session>, Error> {
    let filter = if include_ended {
        "1"
    } else {
        "s.state <> 'ended'"
    };
    query_sessions(connection, filter, ()).await
}

pub(crate) async fn sessions_in(
    connection: &Connection,
    states: &[&str],
) -> Result<Vec<Session>, Error> {
    let placeholders: Vec<String> = (1..=states.len()).map(|n| format!("?{n}")).collect();
    let values: Vec<Value> = states
        .iter()
        .map(|state| Value::Text((*state).to_owned()))
        .collect();
    query_sessions(
        connection,
        &format!("s.state IN ({})", placeholders.join(", ")),
        cockpit_turso::sql::params_from_iter(values),
    )
    .await
}

pub(crate) async fn session(connection: &Connection, id: &SessionId) -> Result<Session, Error> {
    query_sessions(connection, "s.id = ?1", [id.as_str()])
        .await?
        .pop()
        .ok_or_else(|| Error::UnknownSession {
            session: id.clone(),
        })
}

pub(crate) struct NewSession<'values> {
    pub(crate) id: &'values SessionId,
    pub(crate) provider: &'values ProviderName,
    pub(crate) checkout: CheckoutId,
    pub(crate) holds_checkout: bool,
    pub(crate) working_dir: &'values Path,
}

/// Records a new session, `launching` its first launch.
pub(crate) async fn insert_session(
    write: &WriteTransaction<'_>,
    new: &NewSession<'_>,
) -> Result<(), Error> {
    let at = now();
    write
        .execute(
            "INSERT INTO sessions (id, provider, checkout_id, holds_checkout, working_dir,
                 state, revision, created_at, changed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'launching', 1, ?6, ?6)",
            (
                new.id.as_str(),
                new.provider.as_str(),
                new.checkout.0,
                i64::from(new.holds_checkout),
                path_text(new.working_dir)?,
                at,
            ),
        )
        .await?;
    record_transition(write, new.id, None, SessionState::Launching, None).await
}

async fn record_transition(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    from: Option<SessionState>,
    to: SessionState,
    reason: Option<&str>,
) -> Result<(), Error> {
    write
        .execute(
            "INSERT INTO session_transitions (session_id, from_state, to_state, reason, at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            (
                id.as_str(),
                optional_text(from.map(|state| state.to_string()).as_deref()),
                to.to_string(),
                optional_text(reason),
                now(),
            ),
        )
        .await?;
    Ok(())
}

/// Moves the session from one of `from` to `to`, counting a revision and recording the
/// transition. A session in another state is refused for `operation`, unchanged.
pub(crate) async fn transition(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    from: &[SessionState],
    to: SessionState,
    reason: Option<&str>,
    operation: &'static str,
) -> Result<Session, Error> {
    let current = session(write, id).await?;
    if !from.contains(&current.state) {
        return Err(Error::WrongState {
            session: id.clone(),
            state: current.state,
            operation,
        });
    }
    write
        .execute(
            "UPDATE sessions SET state = ?2, stopping_to = ?3, reason = ?4,
                 revision = revision + 1, changed_at = ?5
             WHERE id = ?1",
            (
                id.as_str(),
                to.column(),
                optional_text(to.stop_column()),
                optional_text(reason),
                now(),
            ),
        )
        .await?;
    record_transition(write, id, Some(current.state), to, reason).await?;
    session(write, id).await
}

/// When the session last moved to asleep, if it is asleep now.
pub(crate) async fn entered_asleep_at(
    connection: &Connection,
    id: &SessionId,
) -> Result<Option<i64>, Error> {
    if session(connection, id).await?.state != SessionState::Asleep {
        return Ok(None);
    }
    let mut rows = connection
        .query(
            "SELECT at FROM session_transitions
             WHERE session_id = ?1 AND to_state = 'asleep'
             ORDER BY id DESC LIMIT 1",
            [id.as_str()],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Err(Error::InvalidStoredRow {
            reason: format!("session {id} is asleep with no transition to asleep"),
        }),
    }
}

pub(crate) async fn insert_launch(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    kind: LaunchKind,
    execution: ExecutionId,
) -> Result<(), Error> {
    write
        .execute(
            "INSERT INTO session_launches (session_id, kind, execution_id, launched_at)
             VALUES (?1, ?2, ?3, ?4)",
            (id.as_str(), kind.column(), execution.0, now()),
        )
        .await?;
    Ok(())
}

pub(crate) async fn set_conversation(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    conversation: &ConversationId,
) -> Result<(), Error> {
    write
        .execute(
            "UPDATE sessions SET conversation = ?2 WHERE id = ?1",
            (id.as_str(), conversation.as_str()),
        )
        .await?;
    Ok(())
}

pub(crate) async fn set_design_link(
    write: &WriteTransaction<'_>,
    id: &SessionId,
    link: &str,
) -> Result<Session, Error> {
    session(write, id).await?;
    write
        .execute(
            "UPDATE sessions SET design_link = ?2, revision = revision + 1, changed_at = ?3
             WHERE id = ?1",
            (id.as_str(), link, now()),
        )
        .await?;
    session(write, id).await
}

fn session_from_row(row: &Row) -> Result<Session, Error> {
    let id_text: String = row.get(0)?;
    let invalid = |reason: String| Error::InvalidStoredRow {
        reason: format!("session {id_text}: {reason}"),
    };
    let id = SessionId::parse(&id_text).map_err(|error| invalid(error.to_string()))?;
    let provider =
        ProviderName::new(row.get::<String>(1)?).map_err(|error| invalid(error.to_string()))?;
    let conversation = row
        .get::<Option<String>>(5)?
        .map(ConversationId::new)
        .transpose()
        .map_err(|error| invalid(error.to_string()))?;
    let state_text: String = row.get(7)?;
    let stopping_to: Option<String> = row.get(8)?;
    let state = SessionState::from_columns(&state_text, stopping_to.as_deref())
        .ok_or_else(|| invalid(format!("state {state_text:?} to {stopping_to:?}")))?;
    let launch = match row.get::<Option<i64>>(13)? {
        None => None,
        Some(number) => {
            let kind_text: String = row.get(14)?;
            Some(Launch {
                number,
                kind: LaunchKind::from_column(&kind_text)
                    .ok_or_else(|| invalid(format!("launch kind {kind_text:?}")))?,
                execution: ExecutionId(row.get(15)?),
                launched_at: row.get(16)?,
            })
        }
    };
    Ok(Session {
        id,
        provider,
        checkout: CheckoutId(row.get(2)?),
        holds_checkout: row.get::<i64>(3)? == 1,
        working_dir: PathBuf::from(row.get::<String>(4)?),
        conversation,
        design_link: row.get(6)?,
        state,
        reason: row.get(9)?,
        revision: row.get(10)?,
        created_at: row.get(11)?,
        changed_at: row.get(12)?,
        launch,
    })
}
