//! The state database's infrastructure: one Turso file in SQLite-compatible mode with change
//! capture on, a single opener, write and read transactions, the ordered migration runner,
//! online backup and offline restore. Capabilities own their queries and migrations; they
//! receive a [`WriteTransaction`] or [`ReadTransaction`] and run SQL through [`sql`].

mod backup;
mod changes;
mod database;
mod error;
mod migrations;

pub use backup::restore;
pub use changes::{Change, ChangeKind, Commit};
pub use database::{Database, ReadTransaction, WriteTransaction};
pub use error::Error;
pub use migrations::{Migration, MigrationSet, validate_migrations};

/// The SQL API capabilities use inside a transaction. It leaves out `turso::Builder` and
/// `turso::Database` on purpose: only this crate opens the file, so every write has change
/// capture and the single-opener lock.
pub mod sql {
    pub use turso::{
        Connection, Error, IntoParams, Row, Rows, Statement, Value, named_params, params,
        params_from_iter,
    };
}
