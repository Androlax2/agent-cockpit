//! The status file any bar can read (report 19): every session the owner may want to see,
//! what each needs the owner for, and how many need the owner now. The waybar module of the
//! old cockpit read `$XDG_RUNTIME_DIR/cockpit.json`; this file replaces it with a documented,
//! versioned schema (the crate's README).

use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use serde::Serialize;

/// Bumped when a field changes meaning or goes; a new field keeps it, and readers ignore the
/// fields they do not know.
pub const STATUS_SCHEMA: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Status {
    /// The daemon answered the notifier's last request and everything the notifier starts on
    /// is loaded. While false, `sessions` is what was last known, or empty.
    pub reachable: bool,
    /// How many sessions are in the `needs-you` state.
    pub needs_you: usize,
    /// Every session that has not ended, by id.
    pub sessions: Vec<SessionStatus>,
}

/// A session's state as a bar shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionState {
    /// It needs the owner for anything but having finished: a failed session among them.
    NeedsYou,
    Launching,
    Working,
    Waiting,
    /// Running, and its agent has not reported yet.
    Running,
    Stopping,
    Asleep,
    /// A state this version does not know.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionStatus {
    pub id: String,
    /// The label its agent reported, else its id.
    pub name: String,
    /// Its checkout's project, once the change feed named it.
    pub project: Option<String>,
    pub state: SessionState,
    /// What it needs the owner for, most urgent first: `failure`, `hold`, `question`,
    /// `quota`, `finished`.
    pub needs: Vec<&'static str>,
    /// What its agent last reported doing.
    pub doing: Option<String>,
    /// Seconds since 1970 of the last change to the session or its agent's report.
    pub changed_at: i64,
}

#[derive(Serialize)]
struct StatusFile<'status> {
    schema: u32,
    /// Seconds since 1970. The notifier writes at least every five seconds while it runs,
    /// so an older file means it stopped.
    written_at: i64,
    #[serde(flatten)]
    status: &'status Status,
}

/// Writes `status` to `path` whole: a reader sees the previous file or this one, never a
/// part.
pub fn write_status(path: &Path, status: &Status, written_at: i64) -> io::Result<()> {
    let file = StatusFile {
        schema: STATUS_SCHEMA,
        written_at,
        status,
    };
    let mut text = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
    text.push(b'\n');
    let Some(name) = path.file_name() else {
        return Err(io::Error::other(format!(
            "{}: not a file path",
            path.display()
        )));
    };
    let mut partial_name = name.to_owned();
    partial_name.push(".partial");
    let partial = path.with_file_name(partial_name);
    if let Some(parent) = path.parent() {
        // The daemon makes the runtime directory too, with the same mode; the notifier may
        // start first.
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
    }
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, path)
}
