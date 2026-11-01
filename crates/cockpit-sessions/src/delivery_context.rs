//! The provider command, socket reach and process root kept across daemon restarts.
use crate::{Error, store::path_text};
use cockpit_agent_channel::ProcessIdentity;
use cockpit_provider_api::{ConversationId, LaunchCommand, ProviderError, SessionSockets};
use cockpit_sandbox::{FolderMount, ProviderReach};
use cockpit_turso::{WriteTransaction, sql::Connection};
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::net::UnixStream;
use std::path::Path;

/// The launch identity and sandbox mapping the daemon verifies on each socket connection.
#[derive(Debug, Clone)]
pub struct DeliveryContext {
    pub root: ProcessIdentity,
    pub reach: ProviderReach,
}

/// Injected by the daemon: sessions do not own the kernel's listener verification.
pub type ConnectSessionSocket = fn(&DeliveryContext, &Path) -> Result<UnixStream, ProviderError>;

pub(crate) struct Sockets {
    pub context: Option<DeliveryContext>,
    pub connect: ConnectSessionSocket,
}
impl SessionSockets for Sockets {
    fn connect(&self, path: &Path) -> Result<UnixStream, ProviderError> {
        let context = self.context.as_ref().ok_or_else(|| ProviderError {
            reason: "this launch has no recorded provider socket context; restart the session"
                .into(),
        })?;
        (self.connect)(context, path)
    }
}

pub(crate) async fn record_launch(
    write: &WriteTransaction<'_>,
    execution: i64,
    reach: &ProviderReach,
    launch: &LaunchCommand,
) -> Result<(), Error> {
    let (kind, reason) = match reach {
        ProviderReach::Direct => ("direct", None),
        ProviderReach::Mounted(_) => ("mounted", None),
        ProviderReach::Unreachable { reason, .. } => ("unreachable", Some(reason.as_str())),
    };
    write
        .execute(
            "INSERT INTO session_delivery (execution_id, reach, reason, provider_program, provider_conversation) VALUES (?1, ?2, ?3, ?4, ?5)",
            (execution, kind, reason, launch.program.as_os_str().as_bytes().to_vec(), launch.conversation.as_ref().map(ConversationId::as_str)),
        )
        .await?;
    for (position, argument) in (0_i64..).zip(&launch.arguments) {
        write.execute("INSERT INTO session_delivery_arguments (execution_id, position, value) VALUES (?1, ?2, ?3)", (execution, position, argument.as_bytes().to_vec())).await?;
    }
    for (position, (name, value)) in (0_i64..).zip(&launch.environment) {
        write.execute("INSERT INTO session_delivery_environment (execution_id, position, name, value) VALUES (?1, ?2, ?3, ?4)", (execution, position, name.as_bytes().to_vec(), value.as_bytes().to_vec())).await?;
    }
    if let ProviderReach::Mounted(mounts) = reach {
        for mount in mounts {
            write.execute("INSERT INTO session_delivery_mounts (execution_id, host, sandbox) VALUES (?1, ?2, ?3)", (execution, path_text(&mount.host)?, path_text(&mount.sandbox)?)).await?;
        }
    }
    Ok(())
}

/// Only the original provider command is exposed to the adapter: no Cockpit tokens,
/// inherited owner environment, or reconstruction from the agent's writable directory.
pub(crate) async fn read_launch(
    connection: &Connection,
    execution: i64,
) -> Result<Option<LaunchCommand>, Error> {
    let mut rows = connection.query("SELECT provider_program, provider_conversation FROM session_delivery WHERE execution_id = ?1", [execution]).await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let program = OsString::from_vec(row.get::<Vec<u8>>(0)?).into();
    let conversation = row
        .get::<Option<String>>(1)?
        .map(ConversationId::new)
        .transpose()
        .map_err(|error| Error::InvalidStoredRow {
            reason: error.to_string(),
        })?;
    let mut arguments = Vec::new();
    let mut rows = connection.query("SELECT value FROM session_delivery_arguments WHERE execution_id = ?1 ORDER BY position", [execution]).await?;
    while let Some(row) = rows.next().await? {
        arguments.push(OsString::from_vec(row.get(0)?));
    }
    let mut environment = Vec::new();
    let mut rows = connection.query("SELECT name, value FROM session_delivery_environment WHERE execution_id = ?1 ORDER BY position", [execution]).await?;
    while let Some(row) = rows.next().await? {
        environment.push((
            OsString::from_vec(row.get(0)?),
            OsString::from_vec(row.get(1)?),
        ));
    }
    Ok(Some(LaunchCommand {
        program,
        arguments,
        environment,
        conversation,
    }))
}

pub(crate) async fn record_root(
    write: &WriteTransaction<'_>,
    execution: i64,
    root: &ProcessIdentity,
) -> Result<(), Error> {
    let start = i64::try_from(root.start).map_err(|error| Error::InvalidStoredRow {
        reason: error.to_string(),
    })?;
    write.execute("UPDATE session_delivery SET root_boot = ?2, root_pid = ?3, root_start = ?4 WHERE execution_id = ?1", (execution, root.boot.as_str(), i64::from(root.pid), start)).await?;
    Ok(())
}

pub(crate) async fn read(
    connection: &Connection,
    execution: i64,
) -> Result<Option<DeliveryContext>, Error> {
    let mut rows = connection.query("SELECT reach, reason, root_boot, root_pid, root_start FROM session_delivery WHERE execution_id = ?1", [execution]).await?;
    let Some(row) = rows.next().await? else {
        return Ok(None);
    };
    let (Some(boot), Some(pid), Some(start)) = (
        row.get::<Option<String>>(2)?,
        row.get::<Option<i64>>(3)?,
        row.get::<Option<i64>>(4)?,
    ) else {
        return Ok(None);
    };
    let root = ProcessIdentity {
        boot,
        pid: u32::try_from(pid).map_err(|error| Error::InvalidStoredRow {
            reason: error.to_string(),
        })?,
        start: u64::try_from(start).map_err(|error| Error::InvalidStoredRow {
            reason: error.to_string(),
        })?,
    };
    let reach = match row.get::<String>(0)?.as_str() {
        "direct" => ProviderReach::Direct,
        "unreachable" => ProviderReach::Unreachable {
            feature: cockpit_sandbox::MOUNT_FLAG,
            reason: row.get(1)?,
        },
        "mounted" => {
            let mut mounts = Vec::new();
            let mut rows = connection.query("SELECT host, sandbox FROM session_delivery_mounts WHERE execution_id = ?1 ORDER BY sandbox", [execution]).await?;
            while let Some(row) = rows.next().await? {
                mounts.push(FolderMount {
                    host: row.get::<String>(0)?.into(),
                    sandbox: row.get::<String>(1)?.into(),
                });
            }
            ProviderReach::Mounted(mounts)
        }
        other => {
            return Err(Error::InvalidStoredRow {
                reason: format!("unknown provider reach {other}"),
            });
        }
    };
    Ok(Some(DeliveryContext { root, reach }))
}
