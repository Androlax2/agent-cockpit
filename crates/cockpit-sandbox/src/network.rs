//! Network requests and the owner's decisions on them (#674): a destination the session
//! proxy refused for want of an entry becomes a request the owner allows for the rest of
//! the session, allows always for the project (a revocable grant, #546 D4), or denies. Each
//! decision records when and through which owner client; only the owner decides, so who is
//! the owner. Nothing here adds an entry without a decision: filing a request only asks.
//!
//! The caller owns the transaction and authorization; these functions keep the rules of
//! the rows: one undecided or standing decision per subject and destination, one active
//! grant per project and destination, rows never deleted.

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row, Value};

use crate::egress::{
    AllowEntry, AllowSource, AskedDestination, AskingDestination, DeniedDestination,
    EgressDecisions, Target,
};
use crate::spec::Subject;

/// The owner may not allow a request within this many seconds of it appearing: the
/// keystroke may be meant for what the queue showed before (#156).
pub const NETWORK_ALLOW_GRACE_SECONDS: i64 = 2;

/// The tables, created by the `sandbox` migration set's version 2.
pub(crate) const MIGRATION_SQL: &str = "CREATE TABLE sandbox_network_grants (
        id INTEGER PRIMARY KEY,
        project TEXT NOT NULL CHECK (project <> ''),
        host TEXT NOT NULL CHECK (host <> ''),
        port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
        request INTEGER NOT NULL,
        granted_at INTEGER NOT NULL,
        granted_through TEXT CHECK (granted_through IN ('tui', 'cli')),
        revoked_at INTEGER,
        revoked_through TEXT CHECK (revoked_through IN ('tui', 'cli')),
        CHECK (revoked_at IS NOT NULL OR revoked_through IS NULL)
    );
    CREATE UNIQUE INDEX sandbox_network_grants_one_active
        ON sandbox_network_grants (project, host, port) WHERE revoked_at IS NULL;
    CREATE TABLE sandbox_network_requests (
        id INTEGER PRIMARY KEY,
        subject_kind TEXT NOT NULL CHECK (subject_kind IN ('session')),
        subject TEXT NOT NULL CHECK (subject <> ''),
        project TEXT NOT NULL CHECK (project <> ''),
        host TEXT NOT NULL CHECK (host <> ''),
        port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
        state TEXT NOT NULL CHECK (state IN ('asking', 'allowed-once', 'allowed-always',
            'denied')),
        asked_at INTEGER NOT NULL,
        decided_at INTEGER,
        decided_through TEXT CHECK (decided_through IN ('tui', 'cli')),
        grant_id INTEGER REFERENCES sandbox_network_grants (id),
        CHECK ((state = 'asking') = (decided_at IS NULL)),
        CHECK ((state = 'allowed-always') = (grant_id IS NOT NULL)),
        CHECK (decided_at IS NOT NULL OR decided_through IS NULL)
    );
    CREATE UNIQUE INDEX sandbox_network_requests_one_standing
        ON sandbox_network_requests (subject_kind, subject, host, port)
        WHERE state IN ('asking', 'allowed-once', 'denied');";

/// Where a request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkRequestState {
    Asking,
    /// Allowed for the rest of the asking session.
    AllowedOnce,
    /// Allowed always for the project, through a grant.
    AllowedAlways,
    Denied,
}

impl NetworkRequestState {
    const ALL: [NetworkRequestState; 4] = [
        NetworkRequestState::Asking,
        NetworkRequestState::AllowedOnce,
        NetworkRequestState::AllowedAlways,
        NetworkRequestState::Denied,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            NetworkRequestState::Asking => "asking",
            NetworkRequestState::AllowedOnce => "allowed-once",
            NetworkRequestState::AllowedAlways => "allowed-always",
            NetworkRequestState::Denied => "denied",
        }
    }

    fn parse(text: &str) -> Option<NetworkRequestState> {
        NetworkRequestState::ALL
            .into_iter()
            .find(|state| state.as_str() == text)
    }
}

/// The owner client a decision came through, as that client names itself (#657).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerClient {
    Tui,
    Cli,
}

impl OwnerClient {
    const ALL: [OwnerClient; 2] = [OwnerClient::Tui, OwnerClient::Cli];

    pub fn as_str(self) -> &'static str {
        match self {
            OwnerClient::Tui => "tui",
            OwnerClient::Cli => "cli",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<OwnerClient> {
        OwnerClient::ALL
            .into_iter()
            .find(|client| client.as_str() == text)
    }
}

/// When the owner decided, and through which client; `None` when it did not name itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerDecision {
    pub at: i64,
    pub through: Option<OwnerClient>,
}

/// One destination a session was refused, and what the owner decided. Only sessions ask
/// today; the rows name the subject's kind for #862.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkRequest {
    pub id: i64,
    pub session: String,
    pub project: String,
    pub host: String,
    pub port: u16,
    pub state: NetworkRequestState,
    pub asked_at: i64,
    pub decision: Option<OwnerDecision>,
    /// The grant an "allow always" created.
    pub grant: Option<i64>,
}

/// One project's "allow always" of a destination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkGrant {
    pub id: i64,
    pub project: String,
    pub host: String,
    pub port: u16,
    /// The request whose decision created it.
    pub request: i64,
    pub granted: OwnerDecision,
    pub revoked: Option<OwnerDecision>,
}

/// Why a decision was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkRefusal {
    NotFound {
        kind: &'static str,
        id: i64,
    },
    AlreadyDecided {
        state: &'static str,
    },
    AlreadyRevoked,
    TooSoon {
        wait_seconds: i64,
    },
    /// Only a session asks: a check run's network requests are #862's.
    NotASession,
}

impl std::fmt::Display for NetworkRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkRefusal::NotFound { kind, id } => write!(formatter, "no {kind} {id}"),
            NetworkRefusal::AlreadyDecided { state } => {
                write!(
                    formatter,
                    "the network request was decided already: {state}"
                )
            }
            NetworkRefusal::AlreadyRevoked => {
                formatter.write_str("the network grant was revoked already")
            }
            NetworkRefusal::TooSoon { wait_seconds } => write!(
                formatter,
                "the network request appeared less than {NETWORK_ALLOW_GRACE_SECONDS} s ago; \
                 allow it again in {wait_seconds} s"
            ),
            NetworkRefusal::NotASession => {
                formatter.write_str("only a session's network requests are filed (#862)")
            }
        }
    }
}

#[derive(Debug)]
pub enum NetworkError {
    Refused(NetworkRefusal),
    InvalidRow(String),
    Sql(cockpit_turso::sql::Error),
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkError::Refused(refusal) => refusal.fmt(formatter),
            NetworkError::InvalidRow(reason) => {
                write!(formatter, "a stored network row is invalid: {reason}")
            }
            NetworkError::Sql(error) => write!(formatter, "network requests database: {error}"),
        }
    }
}

impl std::error::Error for NetworkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            NetworkError::Sql(error) => Some(error),
            _ => None,
        }
    }
}

impl From<cockpit_turso::sql::Error> for NetworkError {
    fn from(error: cockpit_turso::sql::Error) -> Self {
        NetworkError::Sql(error)
    }
}

impl From<NetworkRefusal> for NetworkError {
    fn from(refusal: NetworkRefusal) -> Self {
        NetworkError::Refused(refusal)
    }
}

const REQUEST_COLUMNS: &str = "id, subject_kind, subject, project, host, port, state, asked_at,
    decided_at, decided_through, grant_id";
const GRANT_COLUMNS: &str = "id, project, host, port, request, granted_at, granted_through,
    revoked_at, revoked_through";

/// Files `asked` as a request of `subject` in `project`, unless one already stands for it
/// (asking, allowed for the session, denied) or the project holds a grant for it: the
/// subject was refused before the decision reached its launcher. Returns the new request.
/// Only asks: no entry is added until the owner decides.
pub async fn file_network_request(
    write: &WriteTransaction<'_>,
    subject: &Subject,
    project: &str,
    asked: &AskedDestination,
    now: i64,
) -> Result<Option<NetworkRequest>, NetworkError> {
    let (subject_kind, subject_id) = subject_columns(subject)?;
    let port = i64::from(asked.port);
    let mut standing = write
        .query(
            "SELECT 1 FROM sandbox_network_requests
             WHERE subject_kind = ?1 AND subject = ?2 AND host = ?3 AND port = ?4
                AND state IN ('asking', 'allowed-once', 'denied')
             UNION ALL
             SELECT 1 FROM sandbox_network_grants
             WHERE project = ?5 AND host = ?3 AND port = ?4 AND revoked_at IS NULL",
            (subject_kind, subject_id, asked.host.as_str(), port, project),
        )
        .await?;
    if standing.next().await?.is_some() {
        return Ok(None);
    }
    drop(standing);
    let row = write
        .prepare(&format!(
            "INSERT INTO sandbox_network_requests
                (subject_kind, subject, project, host, port, state, asked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'asking', ?6)
             RETURNING {REQUEST_COLUMNS}"
        ))
        .await?
        .query_row((
            subject_kind,
            subject_id,
            project,
            asked.host.as_str(),
            port,
            now,
        ))
        .await?;
    request_from(&row).map(Some)
}

/// Every request still asking, oldest first.
pub async fn asking_network_requests(
    connection: &Connection,
) -> Result<Vec<NetworkRequest>, NetworkError> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {REQUEST_COLUMNS} FROM sandbox_network_requests
                 WHERE state = 'asking' ORDER BY asked_at, id"
            ),
            (),
        )
        .await?;
    let mut requests = Vec::new();
    while let Some(row) = rows.next().await? {
        requests.push(request_from(&row)?);
    }
    Ok(requests)
}

pub async fn network_request(
    connection: &Connection,
    id: i64,
) -> Result<NetworkRequest, NetworkError> {
    let mut rows = connection
        .query(
            &format!("SELECT {REQUEST_COLUMNS} FROM sandbox_network_requests WHERE id = ?1"),
            [id],
        )
        .await?;
    match rows.next().await? {
        Some(row) => request_from(&row),
        None => Err(NetworkRefusal::NotFound {
            kind: "network request",
            id,
        }
        .into()),
    }
}

/// The owner allows request `id`: for the rest of its session, or `always` for its project
/// through a new grant. Refused once decided, and within [`NETWORK_ALLOW_GRACE_SECONDS`] of
/// the request appearing.
pub async fn allow_network_request(
    write: &WriteTransaction<'_>,
    id: i64,
    always: bool,
    through: Option<OwnerClient>,
    now: i64,
) -> Result<NetworkRequest, NetworkError> {
    let request = undecided(write, id).await?;
    let wait_seconds = request.asked_at + NETWORK_ALLOW_GRACE_SECONDS - now;
    if wait_seconds > 0 {
        return Err(NetworkRefusal::TooSoon { wait_seconds }.into());
    }
    let through = through.map(OwnerClient::as_str);
    if !always {
        return decided(
            write,
            "UPDATE sandbox_network_requests
             SET state = 'allowed-once', decided_at = ?2, decided_through = ?3 WHERE id = ?1",
            (id, now, through),
        )
        .await;
    }
    // Two sessions of the project asked before either was decided: the first "always" made
    // the grant, which this one joins instead of making a second.
    let mut granted = write
        .query(
            "SELECT id FROM sandbox_network_grants
             WHERE project = ?1 AND host = ?2 AND port = ?3 AND revoked_at IS NULL",
            (
                request.project.as_str(),
                request.host.as_str(),
                i64::from(request.port),
            ),
        )
        .await?;
    let active: Option<i64> = match granted.next().await? {
        Some(row) => Some(row.get(0)?),
        None => None,
    };
    drop(granted);
    let grant: i64 = match active {
        Some(grant) => grant,
        None => new_grant(write, &request, now, through).await?,
    };
    decided(
        write,
        "UPDATE sandbox_network_requests
         SET state = 'allowed-always', decided_at = ?2, decided_through = ?3, grant_id = ?4
         WHERE id = ?1",
        (id, now, through, grant),
    )
    .await
}

async fn new_grant(
    write: &WriteTransaction<'_>,
    request: &NetworkRequest,
    now: i64,
    through: Option<&str>,
) -> Result<i64, NetworkError> {
    let grant: i64 = write
        .prepare(
            "INSERT INTO sandbox_network_grants
                (project, host, port, request, granted_at, granted_through)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
        )
        .await?
        .query_row((
            request.project.as_str(),
            request.host.as_str(),
            i64::from(request.port),
            request.id,
            now,
            through,
        ))
        .await?
        .get(0)?;
    Ok(grant)
}

/// The owner denies request `id`: its session keeps being refused that destination, now
/// naming the denial, and asks no more.
pub async fn deny_network_request(
    write: &WriteTransaction<'_>,
    id: i64,
    through: Option<OwnerClient>,
    now: i64,
) -> Result<NetworkRequest, NetworkError> {
    undecided(write, id).await?;
    decided(
        write,
        "UPDATE sandbox_network_requests
         SET state = 'denied', decided_at = ?2, decided_through = ?3 WHERE id = ?1",
        (id, now, through.map(OwnerClient::as_str)),
    )
    .await
}

/// The active grants, of `project` or of every project, oldest first.
pub async fn network_grants(
    connection: &Connection,
    project: Option<&str>,
) -> Result<Vec<NetworkGrant>, NetworkError> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {GRANT_COLUMNS} FROM sandbox_network_grants
                 WHERE revoked_at IS NULL AND (?1 IS NULL OR project = ?1) ORDER BY id"
            ),
            [project.map_or(Value::Null, |name| Value::Text(name.to_owned()))],
        )
        .await?;
    let mut grants = Vec::new();
    while let Some(row) = rows.next().await? {
        grants.push(grant_from(&row)?);
    }
    Ok(grants)
}

/// The owner revokes grant `id`; the row stays as the record of who allowed what.
pub async fn revoke_network_grant(
    write: &WriteTransaction<'_>,
    id: i64,
    through: Option<OwnerClient>,
    now: i64,
) -> Result<NetworkGrant, NetworkError> {
    let mut rows = write
        .query(
            &format!("SELECT {GRANT_COLUMNS} FROM sandbox_network_grants WHERE id = ?1"),
            [id],
        )
        .await?;
    let grant = match rows.next().await? {
        Some(row) => grant_from(&row)?,
        None => {
            return Err(NetworkRefusal::NotFound {
                kind: "network grant",
                id,
            }
            .into());
        }
    };
    drop(rows);
    if grant.revoked.is_some() {
        return Err(NetworkRefusal::AlreadyRevoked.into());
    }
    let row = write
        .prepare(&format!(
            "UPDATE sandbox_network_grants SET revoked_at = ?2, revoked_through = ?3
             WHERE id = ?1 RETURNING {GRANT_COLUMNS}"
        ))
        .await?
        .query_row((id, now, through.map(OwnerClient::as_str)))
        .await?;
    grant_from(&row)
}

/// What `subject`'s launcher enforces beside its spec: `project`'s active grants, the
/// destinations the owner allowed the subject for the session, those it denied, and the
/// requests still waiting on the owner.
pub async fn egress_decisions(
    connection: &Connection,
    project: &str,
    subject: &Subject,
) -> Result<EgressDecisions, NetworkError> {
    let (subject_kind, subject_id) = subject_columns(subject)?;
    let mut decisions = EgressDecisions::default();
    for grant in network_grants(connection, Some(project)).await? {
        decisions.allowed.push(AllowEntry {
            target: Target::Name(grant.host),
            port: grant.port,
            source: AllowSource::Grant {
                grant: grant.id.to_string(),
            },
        });
    }
    let mut rows = connection
        .query(
            &format!(
                "SELECT {REQUEST_COLUMNS} FROM sandbox_network_requests
                 WHERE subject_kind = ?1 AND subject = ?2
                    AND state IN ('allowed-once', 'denied', 'asking')
                 ORDER BY id"
            ),
            (subject_kind, subject_id),
        )
        .await?;
    while let Some(row) = rows.next().await? {
        let request = request_from(&row)?;
        match request.state {
            NetworkRequestState::AllowedOnce => decisions.allowed.push(AllowEntry {
                target: Target::Name(request.host),
                port: request.port,
                source: AllowSource::Request {
                    request: request.id,
                },
            }),
            NetworkRequestState::Asking => decisions.asking.push(AskingDestination {
                host: request.host,
                port: request.port,
                request: request.id,
            }),
            NetworkRequestState::Denied => decisions.denied.push(DeniedDestination {
                host: request.host,
                port: request.port,
                request: request.id,
            }),
            // Not selected: its grant is listed above.
            NetworkRequestState::AllowedAlways => {}
        }
    }
    Ok(decisions)
}

async fn undecided(write: &WriteTransaction<'_>, id: i64) -> Result<NetworkRequest, NetworkError> {
    let request = network_request(write, id).await?;
    if request.state != NetworkRequestState::Asking {
        return Err(NetworkRefusal::AlreadyDecided {
            state: request.state.as_str(),
        }
        .into());
    }
    Ok(request)
}

async fn decided(
    write: &WriteTransaction<'_>,
    update: &str,
    params: impl cockpit_turso::sql::IntoParams,
) -> Result<NetworkRequest, NetworkError> {
    let row = write
        .prepare(&format!("{update} RETURNING {REQUEST_COLUMNS}"))
        .await?
        .query_row(params)
        .await?;
    request_from(&row)
}

fn subject_columns(subject: &Subject) -> Result<(&'static str, &str), NetworkError> {
    match subject {
        Subject::Session { session } => Ok(("session", session.as_str())),
        Subject::Check { .. } => Err(NetworkError::Refused(NetworkRefusal::NotASession)),
    }
}

fn request_from(row: &Row) -> Result<NetworkRequest, NetworkError> {
    let subject_kind: String = row.get(1)?;
    if subject_kind != "session" {
        return Err(NetworkError::InvalidRow(format!(
            "subject kind {subject_kind:?}"
        )));
    }
    let state: String = row.get(6)?;
    let state = NetworkRequestState::parse(&state)
        .ok_or_else(|| NetworkError::InvalidRow(format!("request state {state:?}")))?;
    Ok(NetworkRequest {
        id: row.get(0)?,
        session: row.get(2)?,
        project: row.get(3)?,
        host: row.get(4)?,
        port: port_from(row, 5)?,
        state,
        asked_at: row.get(7)?,
        decision: decision_from(row, 8)?,
        grant: row.get(10)?,
    })
}

fn grant_from(row: &Row) -> Result<NetworkGrant, NetworkError> {
    Ok(NetworkGrant {
        id: row.get(0)?,
        project: row.get(1)?,
        host: row.get(2)?,
        port: port_from(row, 3)?,
        request: row.get(4)?,
        granted: decision_from(row, 5)?
            .ok_or_else(|| NetworkError::InvalidRow("a grant without its time".to_owned()))?,
        revoked: decision_from(row, 7)?,
    })
}

fn port_from(row: &Row, column: usize) -> Result<u16, NetworkError> {
    let port: i64 = row.get(column)?;
    u16::try_from(port).map_err(|_| NetworkError::InvalidRow(format!("port {port}")))
}

/// The decision stored in `at, through` from `first`; `None` while `at` is null.
fn decision_from(row: &Row, first: usize) -> Result<Option<OwnerDecision>, NetworkError> {
    let Some(at) = row.get::<Option<i64>>(first)? else {
        return Ok(None);
    };
    let through = match row.get::<Option<String>>(first + 1)? {
        Some(text) => Some(
            OwnerClient::parse(&text)
                .ok_or_else(|| NetworkError::InvalidRow(format!("decided through {text:?}")))?,
        ),
        None => None,
    };
    Ok(Some(OwnerDecision { at, through }))
}
