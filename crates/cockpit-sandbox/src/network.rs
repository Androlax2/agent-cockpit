//! Network requests and the owner's decisions on them (#674): a destination the session
//! proxy refused for want of an entry, or a declared service whose forward the launcher
//! closed for want of a decision (#880), becomes a request the owner allows for the rest of
//! the session, allows always for the project (a revocable grant, #546 D4), or denies. A
//! service's request and grant name its whole record, so a changed declaration asks again. Each
//! decision records when and through which owner client; only the owner decides, so who is
//! the owner. Nothing here adds an entry without a decision: filing a request only asks.
//!
//! The caller owns the transaction and authorization; these functions keep the rules of
//! the rows: one undecided or standing decision per subject and target, one active grant per
//! project and target, rows never deleted.

use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row, Value};

use crate::egress::{
    AllowEntry, AllowSource, AskedDestination, AskingDestination, DeniedDestination,
    EgressDecisions, Target,
};
use crate::forwards::{AllowedService, AskedService, DecidedService};
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

/// The `sandbox` migration set's version 3 (#1256): a grant's `request` may be null, for the
/// github.com grant the owner makes directly, without a request.
pub(crate) const DIRECT_GRANTS_MIGRATION_SQL: &str = "CREATE TABLE sandbox_network_grants_direct (
        id INTEGER PRIMARY KEY,
        project TEXT NOT NULL CHECK (project <> ''),
        host TEXT NOT NULL CHECK (host <> ''),
        port INTEGER NOT NULL CHECK (port BETWEEN 1 AND 65535),
        request INTEGER,
        granted_at INTEGER NOT NULL,
        granted_through TEXT CHECK (granted_through IN ('tui', 'cli')),
        revoked_at INTEGER,
        revoked_through TEXT CHECK (revoked_through IN ('tui', 'cli')),
        CHECK (revoked_at IS NOT NULL OR revoked_through IS NULL)
    );
    INSERT INTO sandbox_network_grants_direct (id, project, host, port, request, granted_at,
            granted_through, revoked_at, revoked_through)
        SELECT id, project, host, port, request, granted_at, granted_through, revoked_at,
            revoked_through
        FROM sandbox_network_grants;
    DROP TABLE sandbox_network_grants;
    ALTER TABLE sandbox_network_grants_direct RENAME TO sandbox_network_grants;
    CREATE UNIQUE INDEX sandbox_network_grants_one_active
        ON sandbox_network_grants (project, host, port) WHERE revoked_at IS NULL;";

/// The `sandbox` migration set's version 5 (#880): a request and a grant name a host and port
/// or, by `kind`, a declared service's name and whole record; the ids stay one sequence, so
/// the owner's verbs name either kind.
pub(crate) const SERVICES_MIGRATION_SQL: &str = "CREATE TABLE sandbox_network_grants_kinds (
        id INTEGER PRIMARY KEY,
        project TEXT NOT NULL CHECK (project <> ''),
        kind TEXT NOT NULL CHECK (kind IN ('host', 'service')),
        host TEXT CHECK (host <> ''),
        port INTEGER CHECK (port BETWEEN 1 AND 65535),
        service TEXT CHECK (service <> ''),
        record TEXT CHECK (record <> ''),
        request INTEGER,
        granted_at INTEGER NOT NULL,
        granted_through TEXT CHECK (granted_through IN ('tui', 'cli')),
        revoked_at INTEGER,
        revoked_through TEXT CHECK (revoked_through IN ('tui', 'cli')),
        CHECK (revoked_at IS NOT NULL OR revoked_through IS NULL),
        CHECK ((kind = 'host') = (host IS NOT NULL)),
        CHECK ((kind = 'host') = (port IS NOT NULL)),
        CHECK ((kind = 'service') = (service IS NOT NULL)),
        CHECK ((kind = 'service') = (record IS NOT NULL))
    );
    INSERT INTO sandbox_network_grants_kinds (id, project, kind, host, port, request,
            granted_at, granted_through, revoked_at, revoked_through)
        SELECT id, project, 'host', host, port, request, granted_at, granted_through,
            revoked_at, revoked_through
        FROM sandbox_network_grants;
    DROP TABLE sandbox_network_grants;
    ALTER TABLE sandbox_network_grants_kinds RENAME TO sandbox_network_grants;
    CREATE UNIQUE INDEX sandbox_network_grants_one_active
        ON sandbox_network_grants (project, host, port)
        WHERE kind = 'host' AND revoked_at IS NULL;
    CREATE UNIQUE INDEX sandbox_network_grants_one_active_service
        ON sandbox_network_grants (project, service, record)
        WHERE kind = 'service' AND revoked_at IS NULL;
    CREATE TABLE sandbox_network_requests_kinds (
        id INTEGER PRIMARY KEY,
        subject_kind TEXT NOT NULL CHECK (subject_kind IN ('session')),
        subject TEXT NOT NULL CHECK (subject <> ''),
        project TEXT NOT NULL CHECK (project <> ''),
        kind TEXT NOT NULL CHECK (kind IN ('host', 'service')),
        host TEXT CHECK (host <> ''),
        port INTEGER CHECK (port BETWEEN 1 AND 65535),
        service TEXT CHECK (service <> ''),
        record TEXT CHECK (record <> ''),
        state TEXT NOT NULL CHECK (state IN ('asking', 'allowed-once', 'allowed-always',
            'denied')),
        asked_at INTEGER NOT NULL,
        decided_at INTEGER,
        decided_through TEXT CHECK (decided_through IN ('tui', 'cli')),
        grant_id INTEGER REFERENCES sandbox_network_grants (id),
        CHECK ((state = 'asking') = (decided_at IS NULL)),
        CHECK ((state = 'allowed-always') = (grant_id IS NOT NULL)),
        CHECK (decided_at IS NOT NULL OR decided_through IS NULL),
        CHECK ((kind = 'host') = (host IS NOT NULL)),
        CHECK ((kind = 'host') = (port IS NOT NULL)),
        CHECK ((kind = 'service') = (service IS NOT NULL)),
        CHECK ((kind = 'service') = (record IS NOT NULL))
    );
    INSERT INTO sandbox_network_requests_kinds (id, subject_kind, subject, project, kind, host,
            port, state, asked_at, decided_at, decided_through, grant_id)
        SELECT id, subject_kind, subject, project, 'host', host, port, state, asked_at,
            decided_at, decided_through, grant_id
        FROM sandbox_network_requests;
    DROP TABLE sandbox_network_requests;
    ALTER TABLE sandbox_network_requests_kinds RENAME TO sandbox_network_requests;
    CREATE UNIQUE INDEX sandbox_network_requests_one_standing
        ON sandbox_network_requests (subject_kind, subject, host, port)
        WHERE kind = 'host' AND state IN ('asking', 'allowed-once', 'denied');
    CREATE UNIQUE INDEX sandbox_network_requests_one_standing_service
        ON sandbox_network_requests (subject_kind, subject, service, record)
        WHERE kind = 'service' AND state IN ('asking', 'allowed-once', 'denied');";

/// The `sandbox` migration set's version 6 (#880): a request an agent filed ahead with
/// `torlek ask service` carries the agent's reason; a launcher's request carries none.
pub(crate) const REASON_MIGRATION_SQL: &str =
    "ALTER TABLE sandbox_network_requests ADD COLUMN reason TEXT CHECK (reason <> '');";

/// The `sandbox` migration set's version 7 (#352, #862 Q2): a check run files requests as a
/// session does; its row names the run as the subject and the check in `subject_check`. One
/// request stands per project, check, host and port, and none is allowed once.
pub(crate) const CHECK_SUBJECTS_MIGRATION_SQL: &str =
    "CREATE TABLE sandbox_network_requests_checks (
        id INTEGER PRIMARY KEY,
        subject_kind TEXT NOT NULL CHECK (subject_kind IN ('session', 'check')),
        subject TEXT NOT NULL CHECK (subject <> ''),
        subject_check TEXT CHECK (subject_check <> ''),
        project TEXT NOT NULL CHECK (project <> ''),
        kind TEXT NOT NULL CHECK (kind IN ('host', 'service')),
        host TEXT CHECK (host <> ''),
        port INTEGER CHECK (port BETWEEN 1 AND 65535),
        service TEXT CHECK (service <> ''),
        record TEXT CHECK (record <> ''),
        state TEXT NOT NULL CHECK (state IN ('asking', 'allowed-once', 'allowed-always',
            'denied')),
        asked_at INTEGER NOT NULL,
        decided_at INTEGER,
        decided_through TEXT CHECK (decided_through IN ('tui', 'cli')),
        grant_id INTEGER REFERENCES sandbox_network_grants (id),
        reason TEXT CHECK (reason <> ''),
        CHECK ((state = 'asking') = (decided_at IS NULL)),
        CHECK ((state = 'allowed-always') = (grant_id IS NOT NULL)),
        CHECK (decided_at IS NOT NULL OR decided_through IS NULL),
        CHECK ((kind = 'host') = (host IS NOT NULL)),
        CHECK ((kind = 'host') = (port IS NOT NULL)),
        CHECK ((kind = 'service') = (service IS NOT NULL)),
        CHECK ((kind = 'service') = (record IS NOT NULL)),
        CHECK ((subject_kind = 'check') = (subject_check IS NOT NULL)),
        CHECK (subject_kind = 'session' OR state <> 'allowed-once')
    );
    INSERT INTO sandbox_network_requests_checks (id, subject_kind, subject, project, kind, host,
            port, service, record, state, asked_at, decided_at, decided_through, grant_id,
            reason)
        SELECT id, subject_kind, subject, project, kind, host, port, service, record, state,
            asked_at, decided_at, decided_through, grant_id, reason
        FROM sandbox_network_requests;
    DROP TABLE sandbox_network_requests;
    CREATE UNIQUE INDEX sandbox_network_requests_one_standing_check
        ON sandbox_network_requests_checks (project, subject_check, host, port)
        WHERE subject_kind = 'check' AND kind = 'host' AND state IN ('asking', 'denied');
    ALTER TABLE sandbox_network_requests_checks RENAME TO sandbox_network_requests;
    CREATE UNIQUE INDEX sandbox_network_requests_one_standing
        ON sandbox_network_requests (subject_kind, subject, host, port)
        WHERE kind = 'host' AND state IN ('asking', 'allowed-once', 'denied');
    CREATE UNIQUE INDEX sandbox_network_requests_one_standing_service
        ON sandbox_network_requests (subject_kind, subject, service, record)
        WHERE kind = 'service' AND state IN ('asking', 'allowed-once', 'denied');";

/// The `sandbox` migration set's version 8 (#390, #856 Q4): the proxy of a project's engine
/// files its containers' requests; the row's subject is the project. Like a check's, none is
/// allowed once: the engine outlives sessions, and "once" would last until a reboot with no
/// grant to revoke. They ask for hosts only: an engine's proxy serves no forward.
pub(crate) const CONTAINERS_SUBJECTS_MIGRATION_SQL: &str =
    "CREATE TABLE sandbox_network_requests_containers (
        id INTEGER PRIMARY KEY,
        subject_kind TEXT NOT NULL CHECK (subject_kind IN ('session', 'check', 'containers')),
        subject TEXT NOT NULL CHECK (subject <> ''),
        subject_check TEXT CHECK (subject_check <> ''),
        project TEXT NOT NULL CHECK (project <> ''),
        kind TEXT NOT NULL CHECK (kind IN ('host', 'service')),
        host TEXT CHECK (host <> ''),
        port INTEGER CHECK (port BETWEEN 1 AND 65535),
        service TEXT CHECK (service <> ''),
        record TEXT CHECK (record <> ''),
        state TEXT NOT NULL CHECK (state IN ('asking', 'allowed-once', 'allowed-always',
            'denied')),
        asked_at INTEGER NOT NULL,
        decided_at INTEGER,
        decided_through TEXT CHECK (decided_through IN ('tui', 'cli')),
        grant_id INTEGER REFERENCES sandbox_network_grants (id),
        reason TEXT CHECK (reason <> ''),
        CHECK ((state = 'asking') = (decided_at IS NULL)),
        CHECK ((state = 'allowed-always') = (grant_id IS NOT NULL)),
        CHECK (decided_at IS NOT NULL OR decided_through IS NULL),
        CHECK ((kind = 'host') = (host IS NOT NULL)),
        CHECK ((kind = 'host') = (port IS NOT NULL)),
        CHECK ((kind = 'service') = (service IS NOT NULL)),
        CHECK ((kind = 'service') = (record IS NOT NULL)),
        CHECK ((subject_kind = 'check') = (subject_check IS NOT NULL)),
        CHECK (subject_kind = 'session' OR state <> 'allowed-once'),
        CHECK (subject_kind <> 'containers' OR subject = project),
        CHECK (subject_kind <> 'containers' OR kind = 'host')
    );
    INSERT INTO sandbox_network_requests_containers (id, subject_kind, subject, subject_check,
            project, kind, host, port, service, record, state, asked_at, decided_at,
            decided_through, grant_id, reason)
        SELECT id, subject_kind, subject, subject_check, project, kind, host, port, service,
            record, state, asked_at, decided_at, decided_through, grant_id, reason
        FROM sandbox_network_requests;
    DROP TABLE sandbox_network_requests;
    ALTER TABLE sandbox_network_requests_containers RENAME TO sandbox_network_requests;
    CREATE UNIQUE INDEX sandbox_network_requests_one_standing_check
        ON sandbox_network_requests (project, subject_check, host, port)
        WHERE subject_kind = 'check' AND kind = 'host' AND state IN ('asking', 'denied');
    CREATE UNIQUE INDEX sandbox_network_requests_one_standing
        ON sandbox_network_requests (subject_kind, subject, host, port)
        WHERE kind = 'host' AND state IN ('asking', 'allowed-once', 'denied');
    CREATE UNIQUE INDEX sandbox_network_requests_one_standing_service
        ON sandbox_network_requests (subject_kind, subject, service, record)
        WHERE kind = 'service' AND state IN ('asking', 'allowed-once', 'denied');";

/// What a request asks for and a grant allows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkTarget {
    /// A host and port the session proxy refused.
    Host { host: String, port: u16 },
    /// A declared service's forward (#880): its name and the whole record a grant is bound
    /// to ([`crate::Forward::record`]).
    Service { service: String, record: String },
}

impl NetworkTarget {
    /// `host:port`, or the service's record, as the owner reads it.
    pub fn describe(&self) -> String {
        match self {
            NetworkTarget::Host { host, port } => crate::egress::destination(host, *port),
            NetworkTarget::Service { record, .. } => record.clone(),
        }
    }

    /// The columns `kind, host, port, service, record` hold.
    fn columns(
        &self,
    ) -> (
        &'static str,
        Option<&str>,
        Option<i64>,
        Option<&str>,
        Option<&str>,
    ) {
        match self {
            NetworkTarget::Host { host, port } => {
                ("host", Some(host), Some(i64::from(*port)), None, None)
            }
            NetworkTarget::Service { service, record } => {
                ("service", None, None, Some(service), Some(record))
            }
        }
    }
}

/// The host the owner may grant a project without a request (#1256): the one its GitHub
/// token binds to for fetches, so the token reaches the project's first sandboxed launch.
pub const GITHUB_HOST: &str = "github.com";

/// Docker Hub's pull hosts (#390, decided 2026-11-09, B): its registry, its token service and
/// its blobs' CDN. No default host: a registry takes pushes with a credential the caller
/// supplies, so the owner grants them per project, at its engine's setup.
pub const DOCKER_HUB_HOSTS: [&str; 3] = [
    "registry-1.docker.io",
    "auth.docker.io",
    "production.cloudflare.docker.com",
];

/// Where a request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkRequestState {
    Asking,
    /// Allowed for the rest of the asking session or check run.
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

    fn parse(text: &str) -> Option<OwnerClient> {
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

/// One destination or service a session or a check run was refused, and what the owner
/// decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkRequest {
    pub id: i64,
    pub subject: Subject,
    pub project: String,
    pub target: NetworkTarget,
    pub state: NetworkRequestState,
    pub asked_at: i64,
    pub decision: Option<OwnerDecision>,
    /// The grant an "allow always" created.
    pub grant: Option<i64>,
    /// Why the agent asked, for a service it asked for ahead ([`ask_service`]); `None` for
    /// what a launcher refused.
    pub reason: Option<String>,
}

/// What an agent's ask for a declared service met ([`ask_service`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceAsk {
    /// A new request, waiting on the owner.
    Filed(NetworkRequest),
    /// The session's request for the record already stands: asking, allowed for the session,
    /// or denied.
    Standing(NetworkRequest),
    /// The project's grant already allows the record.
    Granted(NetworkGrant),
}

/// One project's "allow always" of a destination or a service's record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkGrant {
    pub id: i64,
    pub project: String,
    pub target: NetworkTarget,
    /// The request whose decision created it; `None` for the github.com grant the owner made
    /// directly ([`grant_github`]).
    pub request: Option<i64>,
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
    /// The project already holds an active grant of the destination.
    AlreadyGranted {
        grant: i64,
    },
    TooSoon {
        wait_seconds: i64,
    },
    /// Only a session's agent asks ahead for a service; a check run only files what its
    /// launcher refused.
    NotASession,
    /// A check run ends before the owner can decide, so its check's request is allowed
    /// always or denied, never for one run (#352).
    OnceForACheck,
    /// A project's containers' request is allowed always or denied (#390): the engine
    /// outlives sessions, so "once" would last until a reboot with no grant to revoke.
    OnceForContainers,
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
            NetworkRefusal::AlreadyGranted { grant } => {
                write!(formatter, "network grant {grant} already allows it")
            }
            NetworkRefusal::TooSoon { wait_seconds } => write!(
                formatter,
                "the network request appeared less than {NETWORK_ALLOW_GRACE_SECONDS} s ago; \
                 allow it again in {wait_seconds} s"
            ),
            NetworkRefusal::NotASession => {
                formatter.write_str("only a session's agent asks ahead for a service")
            }
            NetworkRefusal::OnceForACheck => formatter
                .write_str("a check's network request is allowed always or denied, not once"),
            NetworkRefusal::OnceForContainers => formatter.write_str(
                "a project's containers' network request is allowed always or denied, not once",
            ),
        }
    }
}

#[derive(Debug)]
pub enum NetworkError {
    Refused(NetworkRefusal),
    InvalidRow(String),
    /// A project's containers asked as another project: the caller's bug, not the owner's
    /// refusal (#390).
    OtherProject {
        containers: String,
        project: String,
    },
    Sql(cockpit_turso::sql::Error),
}

impl std::fmt::Display for NetworkError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NetworkError::Refused(refusal) => refusal.fmt(formatter),
            NetworkError::InvalidRow(reason) => {
                write!(formatter, "a stored network row is invalid: {reason}")
            }
            NetworkError::OtherProject {
                containers,
                project,
            } => write!(
                formatter,
                "the containers of project {containers} asked as project {project}"
            ),
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

const REQUEST_COLUMNS: &str = "id, subject_kind, subject, project, kind, host, port, service,
    record, state, asked_at, decided_at, decided_through, grant_id, reason, subject_check";
const GRANT_COLUMNS: &str = "id, project, kind, host, port, service, record, request,
    granted_at, granted_through, revoked_at, revoked_through";

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
    let target = NetworkTarget::Host {
        host: asked.host.clone(),
        port: asked.port,
    };
    file_request(write, subject, project, &target, now).await
}

/// Files `asked`, the record of a declared service whose forward `subject`'s launcher closed
/// (#880), as [`file_network_request`] files a host. Only asks: a project file declares a
/// service, and no declaration grants it.
pub async fn file_service_request(
    write: &WriteTransaction<'_>,
    subject: &Subject,
    project: &str,
    asked: &AskedService,
    now: i64,
) -> Result<Option<NetworkRequest>, NetworkError> {
    let target = NetworkTarget::Service {
        service: asked.service.clone(),
        record: asked.record.clone(),
    };
    file_request(write, subject, project, &target, now).await
}

/// An agent of `subject` asks ahead for `forward`, a service its project declares (#880),
/// saying why in `reason`, so the owner decides before the first connection is closed. A
/// grant or a standing request for the record is returned instead of a second request.
/// Only asks: the caller checks the agent may ask, and no entry is added until the owner
/// decides.
pub async fn ask_service(
    write: &WriteTransaction<'_>,
    subject: &Subject,
    project: &str,
    forward: &crate::Forward,
    reason: &str,
    now: i64,
) -> Result<ServiceAsk, NetworkError> {
    if !matches!(subject, Subject::Session { .. }) {
        return Err(NetworkError::Refused(NetworkRefusal::NotASession));
    }
    let target = NetworkTarget::Service {
        service: forward.service.clone(),
        record: forward.record(),
    };
    if let Some(grant) = active_grant(write, project, &target).await? {
        return Ok(ServiceAsk::Granted(network_grant(write, grant).await?));
    }
    if let Some(request) = standing_request(write, subject, project, &target).await? {
        return Ok(ServiceAsk::Standing(request));
    }
    insert_request(write, subject, project, &target, Some(reason), now)
        .await
        .map(ServiceAsk::Filed)
}

async fn file_request(
    write: &WriteTransaction<'_>,
    subject: &Subject,
    project: &str,
    target: &NetworkTarget,
    now: i64,
) -> Result<Option<NetworkRequest>, NetworkError> {
    if active_grant(write, project, target).await?.is_some()
        || standing_request(write, subject, project, target)
            .await?
            .is_some()
    {
        return Ok(None);
    }
    insert_request(write, subject, project, target, None, now)
        .await
        .map(Some)
}

/// `subject`'s request for `target` that still decides it: asking, allowed for the session,
/// or denied. A check run's is its check's in `project`, whichever run asked (#352): a run
/// ends before the owner can decide, so a rerun joins the open card and a denial holds.
async fn standing_request(
    connection: &Connection,
    subject: &Subject,
    project: &str,
    target: &NetworkTarget,
) -> Result<Option<NetworkRequest>, NetworkError> {
    let (kind, host, port, service, record) = target.columns();
    let (subject_kind, subject_id) = subject_columns(subject);
    let mut standing = match subject {
        Subject::Session { .. } | Subject::Containers { .. } => {
            connection
                .query(
                    &format!(
                        "SELECT {REQUEST_COLUMNS} FROM sandbox_network_requests
                         WHERE subject_kind = ?1 AND subject = ?2 AND kind = ?3
                            AND host IS ?4 AND port IS ?5 AND service IS ?6 AND record IS ?7
                            AND state IN ('asking', 'allowed-once', 'denied')"
                    ),
                    (subject_kind, subject_id, kind, host, port, service, record),
                )
                .await?
        }
        Subject::Check { check, .. } => {
            connection
                .query(
                    &format!(
                        "SELECT {REQUEST_COLUMNS} FROM sandbox_network_requests
                         WHERE subject_kind = 'check' AND project = ?1 AND subject_check = ?2
                            AND kind = ?3 AND host IS ?4 AND port IS ?5 AND service IS ?6
                            AND record IS ?7 AND state IN ('asking', 'denied')
                         ORDER BY id LIMIT 1"
                    ),
                    (project, check.as_str(), kind, host, port, service, record),
                )
                .await?
        }
    };
    match standing.next().await? {
        Some(row) => request_from(&row).map(Some),
        None => Ok(None),
    }
}

/// What decides `asked` for a run of `subject`'s check now, for its receipt to name (#352):
/// the open or denied request of the check, else `None`, a grant included.
pub async fn standing_check_request(
    connection: &Connection,
    subject: &Subject,
    project: &str,
    asked: &AskedDestination,
) -> Result<Option<NetworkRequest>, NetworkError> {
    let target = NetworkTarget::Host {
        host: asked.host.clone(),
        port: asked.port,
    };
    if active_grant(connection, project, &target).await?.is_some() {
        return Ok(None);
    }
    standing_request(connection, subject, project, &target).await
}

async fn insert_request(
    write: &WriteTransaction<'_>,
    subject: &Subject,
    project: &str,
    target: &NetworkTarget,
    reason: Option<&str>,
    now: i64,
) -> Result<NetworkRequest, NetworkError> {
    if let Subject::Containers { project: asking } = subject
        && asking != project
    {
        return Err(NetworkError::OtherProject {
            containers: asking.clone(),
            project: project.to_owned(),
        });
    }
    let (subject_kind, subject_id) = subject_columns(subject);
    let subject_check = match subject {
        Subject::Session { .. } | Subject::Containers { .. } => None,
        Subject::Check { check, .. } => Some(check.as_str()),
    };
    let (kind, host, port, service, record) = target.columns();
    let row = write
        .prepare(&format!(
            "INSERT INTO sandbox_network_requests
                (subject_kind, subject, project, kind, host, port, service, record, state,
                 asked_at, reason, subject_check)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'asking', ?9, ?10, ?11)
             RETURNING {REQUEST_COLUMNS}"
        ))
        .await?
        .query_row((
            subject_kind,
            subject_id,
            project,
            kind,
            host,
            port,
            service,
            record,
            now,
            reason,
            subject_check,
        ))
        .await?;
    request_from(&row)
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

/// Every request the owner decided, oldest decision first (#1252).
pub async fn decided_network_requests(
    connection: &Connection,
) -> Result<Vec<NetworkRequest>, NetworkError> {
    let mut rows = connection
        .query(
            &format!(
                "SELECT {REQUEST_COLUMNS} FROM sandbox_network_requests
                 WHERE decided_at IS NOT NULL ORDER BY decided_at, id"
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
/// through a new grant. Refused once decided, for a check's request but always, and within
/// [`NETWORK_ALLOW_GRACE_SECONDS`] of the request appearing.
pub async fn allow_network_request(
    write: &WriteTransaction<'_>,
    id: i64,
    always: bool,
    through: Option<OwnerClient>,
    now: i64,
) -> Result<NetworkRequest, NetworkError> {
    let request = undecided(write, id).await?;
    if !always {
        match request.subject {
            Subject::Session { .. } => {}
            Subject::Check { .. } => return Err(NetworkRefusal::OnceForACheck.into()),
            Subject::Containers { .. } => return Err(NetworkRefusal::OnceForContainers.into()),
        }
    }
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
    let grant: i64 = match active_grant(write, &request.project, &request.target).await? {
        Some(grant) => grant,
        None => {
            new_grant(
                write,
                &request.project,
                &request.target,
                Some(request.id),
                now,
                through,
            )
            .await?
        }
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

/// The project's active grant of `target`, if it holds one.
async fn active_grant(
    connection: &Connection,
    project: &str,
    target: &NetworkTarget,
) -> Result<Option<i64>, NetworkError> {
    let (kind, host, port, service, record) = target.columns();
    let mut granted = connection
        .query(
            "SELECT id FROM sandbox_network_grants
             WHERE project = ?1 AND kind = ?2
                AND host IS ?3 AND port IS ?4 AND service IS ?5 AND record IS ?6
                AND revoked_at IS NULL",
            (project, kind, host, port, service, record),
        )
        .await?;
    match granted.next().await? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

async fn new_grant(
    write: &WriteTransaction<'_>,
    project: &str,
    target: &NetworkTarget,
    request: Option<i64>,
    now: i64,
    through: Option<&str>,
) -> Result<i64, NetworkError> {
    let (kind, host, port, service, record) = target.columns();
    let grant: i64 = write
        .prepare(
            "INSERT INTO sandbox_network_grants
                (project, kind, host, port, service, record, request, granted_at,
                 granted_through)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) RETURNING id",
        )
        .await?
        .query_row((
            project, kind, host, port, service, record, request, now, through,
        ))
        .await?
        .get(0)?;
    Ok(grant)
}

/// The owner grants `project` [`GITHUB_HOST`] on 443 always, without a request: the offer
/// `project add` and `init` make (#1256), so the project's GitHub token binds at its
/// sessions' next launch instead of after a refusal, a request and a relaunch. The caller
/// checks that `project` is registered. Refused when an active grant already allows it.
pub async fn grant_github(
    write: &WriteTransaction<'_>,
    project: &str,
    through: Option<OwnerClient>,
    now: i64,
) -> Result<NetworkGrant, NetworkError> {
    let github = NetworkTarget::Host {
        host: GITHUB_HOST.to_owned(),
        port: crate::egress::TLS_PORT,
    };
    if let Some(grant) = active_grant(write, project, &github).await? {
        return Err(NetworkRefusal::AlreadyGranted { grant }.into());
    }
    let id = new_grant(
        write,
        project,
        &github,
        None,
        now,
        through.map(OwnerClient::as_str),
    )
    .await?;
    network_grant(write, id).await
}

/// The owner grants `project` [`DOCKER_HUB_HOSTS`] on 443 always, without a request: the
/// answer `containers record` asks for (#390), so the engine's first pull from Docker Hub
/// passes. One grant per host, each revoked on its own; a host the project holds already is
/// left as it is. The caller checks that `project` is registered. Refused, naming the first,
/// when the project holds them all.
pub async fn grant_docker_hub(
    write: &WriteTransaction<'_>,
    project: &str,
    through: Option<OwnerClient>,
    now: i64,
) -> Result<Vec<NetworkGrant>, NetworkError> {
    let mut held = None;
    let mut granted = Vec::new();
    for host in DOCKER_HUB_HOSTS {
        let target = NetworkTarget::Host {
            host: host.to_owned(),
            port: crate::egress::TLS_PORT,
        };
        if let Some(grant) = active_grant(write, project, &target).await? {
            held.get_or_insert(grant);
            continue;
        }
        let id = new_grant(
            write,
            project,
            &target,
            None,
            now,
            through.map(OwnerClient::as_str),
        )
        .await?;
        granted.push(network_grant(write, id).await?);
    }
    match held {
        Some(grant) if granted.is_empty() => Err(NetworkRefusal::AlreadyGranted { grant }.into()),
        _ => Ok(granted),
    }
}

async fn network_grant(connection: &Connection, id: i64) -> Result<NetworkGrant, NetworkError> {
    let mut rows = connection
        .query(
            &format!("SELECT {GRANT_COLUMNS} FROM sandbox_network_grants WHERE id = ?1"),
            [id],
        )
        .await?;
    match rows.next().await? {
        Some(row) => grant_from(&row),
        None => Err(NetworkRefusal::NotFound {
            kind: "network grant",
            id,
        }
        .into()),
    }
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
    let grant = network_grant(write, id).await?;
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
    let (subject_kind, subject_id) = subject_columns(subject);
    let mut decisions = EgressDecisions::default();
    for grant in network_grants(connection, Some(project)).await? {
        let source = AllowSource::Grant {
            grant: grant.id.to_string(),
        };
        match grant.target {
            NetworkTarget::Host { host, port } => decisions.allowed.push(AllowEntry {
                target: Target::Name(host),
                port,
                source,
            }),
            NetworkTarget::Service { service, record } => {
                decisions.services.allowed.push(AllowedService {
                    live: live_forward(&service, &record),
                    service,
                    record,
                    source,
                });
            }
        }
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
        match request.target {
            NetworkTarget::Host { host, port } => {
                add_host_decision(&mut decisions, request.state, request.id, host, port);
            }
            NetworkTarget::Service { service, record } => {
                add_service_decision(&mut decisions, request.state, request.id, service, record);
            }
        }
    }
    Ok(decisions)
}

fn add_host_decision(
    decisions: &mut EgressDecisions,
    state: NetworkRequestState,
    request: i64,
    host: String,
    port: u16,
) {
    match state {
        NetworkRequestState::AllowedOnce => decisions.allowed.push(AllowEntry {
            target: Target::Name(host),
            port,
            source: AllowSource::Request { request },
        }),
        NetworkRequestState::Asking => decisions.asking.push(AskingDestination {
            host,
            port,
            request,
        }),
        NetworkRequestState::Denied => {
            decisions.denied.push(DeniedDestination {
                host,
                port,
                request,
            });
        }
        // Not selected: its grant is listed as one.
        NetworkRequestState::AllowedAlways => {}
    }
}

/// The forward the launcher opens live for an allowed `record`: a container port's (#390).
fn live_forward(service: &str, record: &str) -> Option<crate::Forward> {
    crate::Forward::container_port_of(record).filter(|forward| forward.service == service)
}

fn add_service_decision(
    decisions: &mut EgressDecisions,
    state: NetworkRequestState,
    request: i64,
    service: String,
    record: String,
) {
    let services = &mut decisions.services;
    match state {
        NetworkRequestState::AllowedOnce => services.allowed.push(AllowedService {
            live: live_forward(&service, &record),
            service,
            record,
            source: AllowSource::Request { request },
        }),
        NetworkRequestState::Asking => services.asking.push(DecidedService {
            service,
            record,
            request,
        }),
        NetworkRequestState::Denied => services.denied.push(DecidedService {
            service,
            record,
            request,
        }),
        // Not selected: its grant is listed as one.
        NetworkRequestState::AllowedAlways => {}
    }
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

/// The `subject_kind, subject` columns: a check run is named by its run, unique to it, and a
/// project's containers by the project.
fn subject_columns(subject: &Subject) -> (&'static str, &str) {
    match subject {
        Subject::Session { session } => ("session", session.as_str()),
        Subject::Check { run, .. } => ("check", run.as_str()),
        Subject::Containers { project } => ("containers", project.as_str()),
    }
}

fn request_from(row: &Row) -> Result<NetworkRequest, NetworkError> {
    let subject_kind: String = row.get(1)?;
    let subject_id: String = row.get(2)?;
    let subject = match subject_kind.as_str() {
        "session" => Subject::Session {
            session: subject_id,
        },
        "containers" => Subject::Containers {
            project: subject_id,
        },
        "check" => Subject::Check {
            run: subject_id,
            check: row.get::<Option<String>>(15)?.ok_or_else(|| {
                NetworkError::InvalidRow("a check run's request without its check".to_owned())
            })?,
        },
        other => {
            return Err(NetworkError::InvalidRow(format!("subject kind {other:?}")));
        }
    };
    let state: String = row.get(9)?;
    let state = NetworkRequestState::parse(&state)
        .ok_or_else(|| NetworkError::InvalidRow(format!("request state {state:?}")))?;
    Ok(NetworkRequest {
        id: row.get(0)?,
        subject,
        project: row.get(3)?,
        target: target_from(row, 4)?,
        state,
        asked_at: row.get(10)?,
        decision: decision_from(row, 11, NetworkError::InvalidRow)?,
        grant: row.get(13)?,
        reason: row.get(14)?,
    })
}

fn grant_from(row: &Row) -> Result<NetworkGrant, NetworkError> {
    Ok(NetworkGrant {
        id: row.get(0)?,
        project: row.get(1)?,
        target: target_from(row, 2)?,
        request: row.get(7)?,
        granted: decision_from(row, 8, NetworkError::InvalidRow)?
            .ok_or_else(|| NetworkError::InvalidRow("a grant without its time".to_owned()))?,
        revoked: decision_from(row, 10, NetworkError::InvalidRow)?,
    })
}

/// The target stored in `kind, host, port, service, record` from `first`.
fn target_from(row: &Row, first: usize) -> Result<NetworkTarget, NetworkError> {
    let kind: String = row.get(first)?;
    match kind.as_str() {
        "host" => {
            let port: i64 = row.get(first + 2)?;
            Ok(NetworkTarget::Host {
                host: row.get(first + 1)?,
                port: u16::try_from(port)
                    .map_err(|_| NetworkError::InvalidRow(format!("port {port}")))?,
            })
        }
        "service" => Ok(NetworkTarget::Service {
            service: row.get(first + 3)?,
            record: row.get(first + 4)?,
        }),
        other => Err(NetworkError::InvalidRow(format!("target kind {other:?}"))),
    }
}

/// The decision stored in `at, through` from `first`; `None` while `at` is null. `invalid`
/// makes the caller's error for a client no `OwnerClient` names.
pub(crate) fn decision_from<E: From<cockpit_turso::sql::Error>>(
    row: &Row,
    first: usize,
    invalid: impl FnOnce(String) -> E,
) -> Result<Option<OwnerDecision>, E> {
    let Some(at) = row.get::<Option<i64>>(first)? else {
        return Ok(None);
    };
    let through = match row.get::<Option<String>>(first + 1)? {
        Some(text) => match OwnerClient::parse(&text) {
            Some(client) => Some(client),
            None => return Err(invalid(format!("decided through {text:?}"))),
        },
        None => None,
    };
    Ok(Some(OwnerDecision { at, through }))
}
