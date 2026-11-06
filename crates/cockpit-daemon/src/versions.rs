//! `version-report` (report 13, #64): what `torlek version --all` asks the running daemon.
//! The release comes from the binary, the schema from the database's migration ledger set
//! against the migrations this binary knows, and each live session's bundle from the launch
//! that started it.

use std::collections::BTreeMap;
use std::sync::Arc;

use cockpit_protocol::{
    ErrorCode, LaunchBundleView, Outcome, ProtocolError, Reply, SchemaOwnerView, SessionBundleView,
    VersionReportView,
};
use cockpit_sessions::Sessions;
use cockpit_turso::{Database, MigrationSet};

/// What every launch records and the report names: the release this daemon was built from,
/// whose helpers install beside it, and the instruction manifest it serves.
pub(crate) fn launch_bundle() -> cockpit_sessions::LaunchBundle {
    cockpit_sessions::LaunchBundle {
        release: cockpit_agent_context::VERSION.to_owned(),
        instruction_manifest: cockpit_agent_context::manifest().id,
    }
}

pub(crate) async fn report(
    database: Arc<Database>,
    sessions: Arc<Sessions>,
    known: &'static [MigrationSet],
) -> Outcome {
    match assemble(&database, &sessions, known).await {
        Ok(report) => Outcome::Ok(Reply::VersionReport {
            report: Box::new(report),
        }),
        Err(error) => {
            tracing::error!(%error, "the version report failed");
            Outcome::Error(ProtocolError {
                code: ErrorCode::Internal,
                message: "the daemon failed; no version report was made".to_owned(),
                unblock: None,
            })
        }
    }
}

async fn assemble(
    database: &Database,
    sessions: &Sessions,
    known: &[MigrationSet],
) -> Result<VersionReportView, Box<dyn std::error::Error + Send + Sync>> {
    let bundle = launch_bundle();
    Ok(VersionReportView {
        release: bundle.release,
        source_revision: option_env!("TORLEK_SOURCE_REVISION").map(str::to_owned),
        instruction_manifest: bundle.instruction_manifest,
        protocol_versions: cockpit_protocol::SUPPORTED_VERSIONS
            .iter()
            .map(|version| version.0)
            .collect(),
        schema: schema(database, known).await?,
        sessions: sessions
            .live_bundles()
            .await?
            .into_iter()
            .map(|live| SessionBundleView {
                session: live.id.to_string(),
                state: crate::sessions::state_view(live.state),
                bundle: live.bundle.map(|bundle| LaunchBundleView {
                    release: bundle.release,
                    instruction_manifest: bundle.instruction_manifest,
                }),
            })
            .collect(),
    })
}

async fn schema(
    database: &Database,
    known: &[MigrationSet],
) -> Result<Vec<SchemaOwnerView>, Box<dyn std::error::Error + Send + Sync>> {
    let read = database.read().await?;
    let mut rows = read
        .query(
            "SELECT owner, max(version) FROM cockpit_migrations GROUP BY owner",
            (),
        )
        .await?;
    let mut applied = BTreeMap::new();
    while let Some(row) = rows.next().await? {
        let owner: String = row.get(0)?;
        let version: i64 = row.get(1)?;
        applied.insert(owner, u32::try_from(version)?);
    }
    Ok(known
        .iter()
        .map(|set| SchemaOwnerView {
            owner: set.owner.to_owned(),
            applied: applied.get(set.owner).copied().unwrap_or(0),
            known: u32::try_from(set.migrations.len())
                .expect("a capability has under 2^32 migrations"),
        })
        .collect())
}
