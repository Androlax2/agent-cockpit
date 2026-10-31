//! Launch tokens and the binding of a launch to the sender's verified process tree.

use std::fmt;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use cockpit_authorization::SessionId;
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::{Connection, Row};

use crate::process::{ProcessError, ProcessIdentity};
use crate::{Error, Refusal};

const TOKEN_BYTES: usize = 32;

/// A one-time secret the launcher hands to the agent it starts. It identifies nobody alone:
/// it binds only from a process in the launch's recorded tree, and is spent by that binding.
#[derive(Clone, PartialEq, Eq)]
pub struct LaunchToken(String);

impl LaunchToken {
    /// The text the launcher passes to the agent.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn generate() -> Result<LaunchToken, Error> {
        let mut bytes = [0u8; TOKEN_BYTES];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut bytes))
            .map_err(Error::Randomness)?;
        Ok(LaunchToken(
            bytes.iter().map(|byte| format!("{byte:02x}")).collect(),
        ))
    }

    fn is_well_formed(text: &str) -> bool {
        text.len() == TOKEN_BYTES * 2
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }
}

/// Never prints the secret.
impl fmt::Debug for LaunchToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LaunchToken(..)")
    }
}

/// Who sent a request, as the kernel says: its process and that process's ancestors, nearest
/// first, or why they could not be read.
pub type PeerAncestry = Result<Vec<ProcessIdentity>, ProcessError>;

/// Starts a launch of `session` and returns the token to pass to its agent. Launching a
/// session again replaces its previous launch, so the old tree and an unspent old token no
/// longer identify it. The launch binds only after [`record_root`].
pub async fn mint(write: &WriteTransaction<'_>, session: &SessionId) -> Result<LaunchToken, Error> {
    let token = LaunchToken::generate()?;
    write
        .execute(
            "DELETE FROM agent_channel_launches WHERE session = ?1",
            [session.as_str()],
        )
        .await?;
    write
        .execute(
            "INSERT INTO agent_channel_launches (session, token, minted_at) VALUES (?1, ?2, ?3)",
            (session.as_str(), token.as_str(), unix_seconds()),
        )
        .await?;
    Ok(token)
}

/// Records the process the launcher started for `session`'s pending launch: its token then
/// binds from that process or any of its descendants.
pub async fn record_root(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    root: &ProcessIdentity,
) -> Result<(), Error> {
    let updated = write
        .execute(
            "UPDATE agent_channel_launches SET root_boot = ?2, root_pid = ?3, root_start = ?4
             WHERE session = ?1 AND bound_at IS NULL",
            (
                session.as_str(),
                root.boot.as_str(),
                i64::from(root.pid),
                start_column(root.start)?,
            ),
        )
        .await?;
    if updated == 0 {
        return Err(Error::NoPendingLaunch {
            session: session.clone(),
        });
    }
    Ok(())
}

/// The session the sender acts for. A well-formed `token` of a pending launch binds that
/// launch when the sender descends from its root, and is spent. Without one, the nearest
/// ancestor that is a bound launch's root names the session, so processes a session starts
/// (nested agents, hooks) act as that session. Nothing in a payload is consulted.
pub async fn identify(
    write: &WriteTransaction<'_>,
    peer: &PeerAncestry,
    token: Option<&str>,
) -> Result<SessionId, Error> {
    let ancestry = verified(peer)?;
    if let Some(token) = well_formed(token)
        && let Some(session) = bind(write, ancestry, token).await?
    {
        return Ok(session);
    }
    bound_session(write, ancestry, token).await
}

/// [`identify`] without binding, for requests that would otherwise write nothing (status
/// polls): it reads, so it runs in a read transaction and never waits for the writer.
/// `None` when a well-formed `token` names a pending launch: only [`identify`], in a write
/// transaction, may bind or refuse it. Every other outcome is the one [`identify`] gives.
pub async fn identify_bound(
    connection: &Connection,
    peer: &PeerAncestry,
    token: Option<&str>,
) -> Result<Option<SessionId>, Error> {
    let ancestry = verified(peer)?;
    if let Some(token) = well_formed(token)
        && is_pending(connection, token).await?
    {
        return Ok(None);
    }
    bound_session(connection, ancestry, token).await.map(Some)
}

fn verified(peer: &PeerAncestry) -> Result<&[ProcessIdentity], Error> {
    peer.as_deref()
        .map_err(|error| Refusal::PeerUnverifiable(error.clone()).into())
}

fn well_formed(token: Option<&str>) -> Option<&str> {
    token.filter(|token| LaunchToken::is_well_formed(token))
}

async fn is_pending(connection: &Connection, token: &str) -> Result<bool, Error> {
    let mut rows = connection
        .query(
            "SELECT 1 FROM agent_channel_launches WHERE token = ?1",
            [token],
        )
        .await?;
    Ok(rows.next().await?.is_some())
}

/// The session whose bound root is the sender's nearest ancestor. `token` only chooses the
/// refusal when none is.
async fn bound_session(
    connection: &Connection,
    ancestry: &[ProcessIdentity],
    token: Option<&str>,
) -> Result<SessionId, Error> {
    // One row per session: read every bound root once, then take the nearest ancestor's.
    let mut rows = connection
        .query(
            "SELECT session, root_boot, root_pid, root_start FROM agent_channel_launches
             WHERE bound_at IS NOT NULL",
            (),
        )
        .await?;
    let mut bound_roots = Vec::new();
    while let Some(row) = rows.next().await? {
        bound_roots.push((
            row.get::<String>(1)?,
            row.get::<i64>(2)?,
            row.get::<i64>(3)?,
            session_from(&row)?,
        ));
    }
    for process in ancestry {
        let (pid, start) = (i64::from(process.pid), start_column(process.start)?);
        if let Some((.., session)) = bound_roots.iter().find(|(boot, root_pid, root_start, _)| {
            *boot == process.boot && *root_pid == pid && *root_start == start
        }) {
            return Ok(session.clone());
        }
    }
    Err(match token {
        Some(_) => Refusal::UnknownLaunchToken,
        None => Refusal::Unidentified,
    }
    .into())
}

/// Binds the pending launch `token` names, or `None` when no launch waits for it. A token
/// presented from outside its tree is refused and stays unspent, so a leaked token cannot be
/// used to lock the real agent out.
async fn bind(
    write: &WriteTransaction<'_>,
    ancestry: &[ProcessIdentity],
    token: &str,
) -> Result<Option<SessionId>, Error> {
    let mut rows = write
        .query(
            "SELECT session, root_boot, root_pid, root_start FROM agent_channel_launches
             WHERE token = ?1",
            [token],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let session = session_from(&row)?;
    let (Some(boot), Some(pid), Some(start)) = (
        row.get::<Option<String>>(1)?,
        row.get::<Option<i64>>(2)?,
        row.get::<Option<i64>>(3)?,
    ) else {
        return Err(Refusal::LaunchNotStarted { session }.into());
    };
    let is_in_tree = ancestry.iter().any(|process| {
        process.boot == boot
            && i64::from(process.pid) == pid
            && start_column(process.start).ok() == Some(start)
    });
    if !is_in_tree {
        return Err(Refusal::OutsideLaunchTree.into());
    }
    write
        .execute(
            "UPDATE agent_channel_launches SET token = NULL, bound_at = ?2 WHERE session = ?1",
            (session.as_str(), unix_seconds()),
        )
        .await?;
    Ok(Some(session))
}

fn session_from(row: &Row) -> Result<SessionId, Error> {
    SessionId::new(row.get::<String>(0)?).map_err(|error| Error::InvalidStoredRow {
        table: "agent_channel_launches",
        reason: error.to_string(),
    })
}

/// SQLite integers are signed; a start time past `i64::MAX` ticks is not a real one.
fn start_column(start: u64) -> Result<i64, Error> {
    i64::try_from(start).map_err(|_| {
        Refusal::PeerUnverifiable(ProcessError::Unreadable {
            reason: format!("start time {start} is out of range"),
        })
        .into()
    })
}

pub(crate) fn unix_seconds() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after 1970");
    i64::try_from(elapsed.as_secs()).expect("seconds since 1970 fit in an i64")
}
