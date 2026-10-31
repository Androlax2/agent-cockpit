//! The state database's infrastructure: one Turso file in SQLite-compatible mode with change
//! capture on, a single opener, write and read transactions, the ordered migration runner,
//! online backup and offline restore. Capabilities own their queries and migrations; they
//! receive a [`WriteTransaction`] or [`ReadTransaction`] and run SQL through the re-exported
//! [`turso`] API, which they never open themselves.

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
pub use turso;
