//! The block of host ports each sandboxed session holds in its project engine's network
//! namespace (#1371), from its first launch until it ends, and the container ports it may
//! therefore open: none of another session's block.

use cockpit_config::PortBlock;
use cockpit_turso::WriteTransaction;
use cockpit_turso::sql::Connection;

use crate::{Error, SessionId};

/// A port block held by a session not ended (#1371).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldPortBlock {
    pub session: SessionId,
    pub project: String,
    pub block: PortBlock,
}

/// The port block `session` of `project` launches with: the one it took at an earlier
/// launch, or else the lowest no session of the project holds. The blocks ended sessions
/// held are freed first. The caller commits.
pub(crate) async fn hold_port_block(
    write: &WriteTransaction<'_>,
    session: &SessionId,
    project: &str,
) -> Result<PortBlock, Error> {
    write
        .execute(
            "DELETE FROM session_port_blocks
             WHERE session_id IN (SELECT id FROM sessions WHERE state = 'ended')",
            (),
        )
        .await?;
    let mut rows = write
        .query(
            "SELECT block FROM session_port_blocks WHERE session_id = ?1",
            [session.as_str()],
        )
        .await?;
    if let Some(row) = rows.next().await? {
        return stored_port_block(row.get(0)?);
    }
    let mut rows = write
        .query(
            "SELECT block FROM session_port_blocks WHERE project = ?1 ORDER BY block",
            [project],
        )
        .await?;
    let mut lowest_free: i64 = 0;
    while let Some(row) = rows.next().await? {
        let held: i64 = row.get(0)?;
        if held != lowest_free {
            break;
        }
        lowest_free += 1;
    }
    let block = u16::try_from(lowest_free)
        .ok()
        .and_then(PortBlock::new)
        .ok_or_else(|| Error::NoPortBlock {
            project: project.to_owned(),
        })?;
    write
        .execute(
            "INSERT INTO session_port_blocks (session_id, project, block) VALUES (?1, ?2, ?3)",
            (session.as_str(), project, i64::from(block.index())),
        )
        .await?;
    Ok(block)
}

/// Every port block a session not ended holds, by project and block.
pub async fn live_port_blocks(connection: &Connection) -> Result<Vec<HeldPortBlock>, Error> {
    let mut rows = connection
        .query(
            "SELECT b.session_id, b.project, b.block
             FROM session_port_blocks b JOIN sessions s ON s.id = b.session_id
             WHERE s.state <> 'ended'
             ORDER BY b.project, b.block",
            (),
        )
        .await?;
    let mut held = Vec::new();
    while let Some(row) = rows.next().await? {
        let id_text: String = row.get(0)?;
        let session = SessionId::parse(&id_text).map_err(|error| Error::InvalidStoredRow {
            reason: format!("port block of session {id_text}: {error}"),
        })?;
        held.push(HeldPortBlock {
            session,
            project: row.get(1)?,
            block: stored_port_block(row.get(2)?)?,
        });
    }
    Ok(held)
}

fn stored_port_block(index: i64) -> Result<PortBlock, Error> {
    u16::try_from(index)
        .ok()
        .and_then(PortBlock::new)
        .ok_or_else(|| Error::InvalidStoredRow {
            reason: format!("port block {index} is not a block"),
        })
}

/// The block of another session of `project` that holds `port`, if one does: that
/// session's containers publish there, so no container forward of `session` opens it.
pub fn other_sessions_block<'held>(
    held: &'held [HeldPortBlock],
    project: &str,
    session: &str,
    port: u16,
) -> Option<&'held HeldPortBlock> {
    let block = PortBlock::containing(port)?;
    held.iter().find(|holder| {
        holder.project == project && holder.block == block && holder.session.as_str() != session
    })
}

/// `services` without the container ports of other sessions' blocks: a port the owner
/// allowed always for the project opens only in the session whose block holds it.
pub fn keep_own_container_ports(
    services: &mut cockpit_sandbox::ServiceDecisions,
    held: &[HeldPortBlock],
    project: &str,
    session: &str,
) {
    services.allowed.retain(|allowed| {
        allowed
            .live
            .as_ref()
            .and_then(cockpit_sandbox::Forward::listen_port)
            .is_none_or(|port| other_sessions_block(held, project, session, port).is_none())
    });
}
