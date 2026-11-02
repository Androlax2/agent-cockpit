//! Bounded owner attestations, separate from engine capability reports. The caller owns
//! authorization and the transaction; the launch's unique proxy identity prevents stale writes.

use std::fmt;

use cockpit_turso::WriteTransaction;

const PREFIX: &str = "owner probe: ";

#[derive(Debug)]
pub enum ObservationError {
    Invalid(&'static str),
    StaleLaunch,
    AlreadyRecorded,
    Sql(cockpit_turso::sql::Error),
}

impl fmt::Display for ObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => write!(formatter, "invalid sandbox observations: {reason}"),
            Self::StaleLaunch => formatter
                .write_str("sandbox observations refer to a stale launch or no sandboxed launch"),
            Self::AlreadyRecorded => formatter.write_str(
                "sandbox observations already recorded for this launch; refusing to replace them",
            ),
            Self::Sql(error) => write!(formatter, "sandbox observations database: {error}"),
        }
    }
}

impl std::error::Error for ObservationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sql(error) => Some(error),
            _ => None,
        }
    }
}

impl From<cockpit_turso::sql::Error> for ObservationError {
    fn from(error: cockpit_turso::sql::Error) -> Self {
        Self::Sql(error)
    }
}

/// Append one immutable batch for the latest sandboxed launch. An identical retry is
/// idempotent. The prefix records the owner's provenance, not independent engine proof.
/// The caller commits only after this function succeeds.
pub async fn record_observations(
    write: &WriteTransaction<'_>,
    session: &str,
    client_identity: &str,
    observations: &[String],
) -> Result<(), ObservationError> {
    if client_identity.is_empty()
        || client_identity.len() > 256
        || client_identity.chars().any(char::is_control)
    {
        return Err(ObservationError::Invalid(
            "client identity must be 1–256 bytes without control characters",
        ));
    }
    if observations.is_empty() || observations.len() > 16 {
        return Err(ObservationError::Invalid("expected 1–16 observations"));
    }
    for (index, observation) in observations.iter().enumerate() {
        if observation.trim().is_empty()
            || observation.len() > 512
            || observation.chars().any(char::is_control)
            || observations[..index].contains(observation)
        {
            return Err(ObservationError::Invalid(
                "each observation must be unique, nonblank, at most 512 bytes and without control characters",
            ));
        }
    }
    let mut rows = write
        .query(
            "SELECT id, enforcement, client_identity FROM sandbox_launches
         WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
            [session],
        )
        .await?;
    let Some(row) = rows.next().await? else {
        return Err(ObservationError::StaleLaunch);
    };
    let launch: i64 = row.get(0)?;
    let enforcement: String = row.get(1)?;
    let identity: Option<String> = row.get(2)?;
    if enforcement != "sandboxed" || identity.as_deref() != Some(client_identity) {
        return Err(ObservationError::StaleLaunch);
    }
    drop(rows);
    let requested: Vec<String> = observations
        .iter()
        .map(|line| format!("{PREFIX}{line}"))
        .collect();
    let mut lines = write.query(
        "SELECT position, kind, text FROM sandbox_launch_lines WHERE launch = ?1 ORDER BY position", [launch],
    ).await?;
    let mut next = 0;
    let mut previous = Vec::new();
    while let Some(row) = lines.next().await? {
        next = row.get::<i64>(0)? + 1;
        let kind: String = row.get(1)?;
        let text: String = row.get(2)?;
        if kind == "observed" && text.starts_with(PREFIX) {
            previous.push(text);
        }
    }
    drop(lines);
    if !previous.is_empty() {
        return if previous == requested {
            Ok(())
        } else {
            Err(ObservationError::AlreadyRecorded)
        };
    }
    for (index, text) in requested.iter().enumerate() {
        write
            .execute(
                "INSERT INTO sandbox_launch_lines (launch, position, kind, text, enforced_by)
             VALUES (?1, ?2, 'observed', ?3, NULL)",
                (launch, next + index as i64, text.as_str()),
            )
            .await?;
    }
    Ok(())
}
