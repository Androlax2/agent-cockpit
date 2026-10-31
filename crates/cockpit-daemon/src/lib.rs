#![doc = include_str!("../README.md")]

mod error;
mod instance;
mod lanes;
mod routes;
mod server;
mod supervisor;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Semaphore;

pub use error::StartError;
pub use instance::{Instance, Options};
pub use lanes::LaneLimits;
pub use routes::{Handler, Job, Route, Routes, WorkClass, refusal};
pub use server::Limits;

/// A started daemon: it accepts requests until dropped or the process exits.
pub struct Daemon {
    socket_path: PathBuf,
    accepting: tokio::task::JoinHandle<()>,
    _database: cockpit_turso::Database,
    _instance: Instance,
}

impl Instance {
    /// Runs the restart contract, then serves `handler` on the socket: open the state
    /// database (durable state loads with it), reconcile, and only then bind and accept
    /// (report 03). Nothing is dispatched before reconciliation has finished.
    pub async fn start(self, handler: Arc<dyn Handler>) -> Result<Daemon, StartError> {
        let options = &self.options;
        let database = cockpit_turso::Database::open(
            &options.database_path(),
            &[cockpit_authorization::MIGRATIONS],
        )
        .await?;
        tracing::info!(epoch = database.epoch(), "opened the state database");
        reconcile(&database);
        let socket_path = options.socket_path();
        let listener = Arc::new(server::bind(&socket_path)?);
        let limits = options.limits;
        let shared = Arc::new(server::Shared {
            owner_uid: self.owner_uid,
            limits,
            lanes: lanes::Lanes::new(limits.control, limits.bulk),
            handler,
            connections: Arc::new(Semaphore::new(limits.connections)),
        });
        let accepting = tokio::spawn(supervisor::supervise("accept", move || {
            server::accept_connections(Arc::clone(&listener), Arc::clone(&shared))
        }));
        tracing::info!(socket = %socket_path.display(), "accepting requests");
        Ok(Daemon {
            socket_path,
            accepting,
            _database: database,
            _instance: self,
        })
    }
}

/// Step 2 and 3 of the restart contract: each capability that holds launch intents,
/// reservations or execution identities reconciles them against the real processes here,
/// before the socket exists. No capability holds any yet (M1 adds processes and sessions).
fn reconcile(_database: &cockpit_turso::Database) {
    tracing::info!("reconciled durable state; nothing to reattach");
}

impl Daemon {
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    /// Serves until the accept loop ends, which only a cancelled runtime does.
    pub async fn wait(mut self) {
        let _ = (&mut self.accepting).await;
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.accepting.abort();
    }
}
