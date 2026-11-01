//! Logging of Cockpit's own operation: levels, runtime overrides, bounded log files and the
//! support bundle (docs/design/greenfield/12-diagnostics-logging.md). Hosted work (prompts,
//! agent output, commands the agent runs, environment values) is never Cockpit's to log.
//!
//! A long-lived process calls [`start`] once and installs [`Diagnostics::dispatch`] as its
//! global default. Records go to `cockpit.log` as JSON lines through a bounded queue, so a
//! stalled disk costs dropped records, counted and reported, never a blocked caller.

mod bundle;
mod error;
mod guard;
mod level;
mod queue;
mod rotation;

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tracing::Dispatch;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{Registry, fmt, reload};

pub use bundle::{
    BundleItem, BundlePlan, BundleRequest, BundleSection, DEFAULT_BUNDLE_MAX_BYTES, plan_bundle,
};
pub use error::Error;
pub use level::{
    ActiveOverride, DEFAULT_OVERRIDE, Directives, LOG_LEVEL_VAR, LevelControl, LevelOverride,
    LevelStatus, MAX_OVERRIDE,
};
pub use rotation::FileLimits;

/// How long dropping [`Diagnostics`] waits for queued records before giving up.
const DROP_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

/// What [`start`] needs; [`Options::new`] fills in the report-12 defaults.
#[derive(Debug, Clone)]
pub struct Options {
    /// The process's own directory, usually from [`log_dir_in`]; created 0700 when missing.
    pub log_dir: PathBuf,
    /// The level the process starts with and every override reverts to.
    pub directives: Directives,
    /// Also print pretty records to stderr (`--foreground`); the file stays JSON lines.
    pub foreground: bool,
    pub limits: FileLimits,
    /// Records waiting for the disk before new ones are dropped.
    pub queue_records: usize,
}

impl Options {
    pub fn new(log_dir: PathBuf, directives: Directives) -> Self {
        Self {
            log_dir,
            directives,
            foreground: false,
            limits: FileLimits::default(),
            queue_records: 4096,
        }
    }
}

/// A started logging pipeline. Dropping it drains the queue for at most one second.
pub struct Diagnostics {
    dispatch: Dispatch,
    level: LevelControl,
    queue: queue::Queue,
    refused: Arc<AtomicU64>,
}

impl std::fmt::Debug for Diagnostics {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Diagnostics")
            .field("dropped_records", &self.dropped_records())
            .field("refused_records", &self.refused_records())
            .finish_non_exhaustive()
    }
}

/// Opens the log file and starts the writer thread. Fails when the directory or file cannot
/// be created or made private, or when the limits are unusable.
pub fn start(options: Options) -> Result<Diagnostics, Error> {
    let file = rotation::RotatingFile::open(&options.log_dir, options.limits)?;
    let queue = queue::Queue::start(file, options.queue_records)?;
    let refused = Arc::new(AtomicU64::new(0));
    let (filter, filter_handle) = reload::Layer::new(options.directives.env_filter());
    let file_layer = fmt::layer()
        .json()
        .with_current_span(false)
        .with_span_list(true)
        .with_writer(queue.writer());
    let stderr_layer = options
        .foreground
        .then(|| fmt::layer().pretty().with_writer(io::stderr));
    let subscriber = Registry::default()
        .with(filter)
        .with(guard::HostedContentGuard::new(
            queue.writer(),
            Arc::clone(&refused),
        ))
        .with(file_layer)
        .with(stderr_layer);
    let dispatch = Dispatch::new(subscriber);
    let level = LevelControl::new(options.directives, filter_handle, queue.writer());
    Ok(Diagnostics {
        dispatch,
        level,
        queue,
        refused,
    })
}

impl Diagnostics {
    /// The subscriber to install, globally with `tracing::dispatcher::set_global_default`.
    pub fn dispatch(&self) -> &Dispatch {
        &self.dispatch
    }

    /// Runtime level control. Callers must be host-only: agents never raise levels.
    pub fn level(&self) -> &LevelControl {
        &self.level
    }

    /// Records lost because the queue was full, the disk failed or a record exceeded a file.
    pub fn dropped_records(&self) -> u64 {
        self.queue.dropped_records()
    }

    /// Records and spans refused because a field name marks hosted content (see `guard`).
    pub fn refused_records(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Asks the writer to write what is queued, then stop; waits at most `timeout` and
    /// returns whether it finished. Records logged afterwards are dropped and counted.
    pub fn shutdown(&self, timeout: Duration) -> bool {
        self.queue.shutdown(timeout)
    }
}

impl Drop for Diagnostics {
    fn drop(&mut self) {
        self.queue.shutdown(DROP_DRAIN_TIMEOUT);
    }
}

/// `<state dir>/logs/<process>`. `state_dir` is the active profile's own state directory
/// (`$XDG_STATE_HOME/cockpit-<name>`, #196), which configuration resolves; `process` names one
/// long-lived process (`daemon`, `tui`): each gets its own `cockpit.log`.
pub fn log_dir_in(state_dir: &Path, process: &str) -> Result<PathBuf, Error> {
    error::check_plain_name("process", process)?;
    Ok(state_dir.join("logs").join(process))
}
