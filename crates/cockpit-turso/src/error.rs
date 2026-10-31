use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// Another `Database` (in this process or another one) holds the file.
    AlreadyOpen {
        path: PathBuf,
    },
    /// The file holds migrations this binary does not know; an older binary must not run on it.
    SchemaNewerThanBinary {
        owner: String,
        applied_version: u32,
        known_version: u32,
    },
    /// An applied migration's SQL differs from the binary's: the schema it describes is not
    /// the schema in the file.
    MigrationEdited {
        owner: String,
        version: u32,
    },
    /// The binary's own migration list breaks the runner's rules.
    InvalidMigrations {
        owner: String,
        reason: String,
    },
    /// A backup or restore would overwrite a file it does not own.
    DestinationExists {
        path: PathBuf,
    },
    /// A file failed `PRAGMA integrity_check`; `report` is the check's output.
    IntegrityCheckFailed {
        path: PathBuf,
        report: String,
    },
    /// A backup or a restore's staged copy kept WAL frames that its main file lacks; the
    /// operation stopped instead of deleting them.
    CheckpointIncomplete {
        path: PathBuf,
    },
    /// Change capture returned a row this crate cannot interpret.
    UnexpectedChange {
        change_id: i64,
        reason: String,
    },
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Database(turso::Error),
}

impl Error {
    /// Maps an I/O failure on `path`, for `map_err`.
    pub(crate) fn io(path: &Path) -> impl FnOnce(std::io::Error) -> Error {
        let path = path.to_path_buf();
        move |source| Error::Io { path, source }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::AlreadyOpen { path } => write!(
                formatter,
                "{} is already open in another Cockpit daemon or tool; stop it first",
                path.display()
            ),
            Error::SchemaNewerThanBinary {
                owner,
                applied_version,
                known_version,
            } => write!(
                formatter,
                "the database holds {owner} schema version {applied_version}, newer than \
                 version {known_version} this Cockpit knows; run the newer Cockpit or restore \
                 a backup taken by this one"
            ),
            Error::MigrationEdited { owner, version } => write!(
                formatter,
                "{owner} migration {version} differs from the one applied to this database"
            ),
            Error::InvalidMigrations { owner, reason } => {
                write!(formatter, "invalid {owner} migrations: {reason}")
            }
            Error::DestinationExists { path } => {
                write!(
                    formatter,
                    "{} already exists; refusing to overwrite it",
                    path.display()
                )
            }
            Error::IntegrityCheckFailed { path, report } => write!(
                formatter,
                "{} failed the integrity check: {report}",
                path.display()
            ),
            Error::CheckpointIncomplete { path } => write!(
                formatter,
                "{} still holds frames its database file lacks; stopped before replacing \
                 anything",
                path.display()
            ),
            Error::UnexpectedChange { change_id, reason } => {
                write!(formatter, "change {change_id}: {reason}")
            }
            Error::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Error::Database(source) => write!(formatter, "database: {source}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Database(source) => Some(source),
            _ => None,
        }
    }
}

impl From<turso::Error> for Error {
    fn from(source: turso::Error) -> Self {
        Error::Database(source)
    }
}
