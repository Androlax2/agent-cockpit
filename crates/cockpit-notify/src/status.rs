//! The status file any bar can read (report 19): every session the owner may want to see,
//! what each needs the owner for, and how many need the owner now. The waybar module of the
//! old cockpit read `$XDG_RUNTIME_DIR/cockpit.json`; this file replaces it with a documented,
//! versioned schema (the crate's README).

use std::io;
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
    /// Every session but the ones that ended and need nothing, by id.
    pub sessions: Vec<SessionStatus>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionStatus {
    pub id: String,
    /// The label its agent reported, else its id.
    pub name: String,
    /// Its checkout's project, once the change feed named it.
    pub project: Option<String>,
    /// `needs-you` when it needs the owner for anything but having finished; else
    /// `launching`, `working`, `waiting`, `running` (no report yet), `stopping`, `asleep`,
    /// `ended` or `unknown`.
    pub state: &'static str,
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
    std::fs::write(&partial, text)?;
    std::fs::rename(&partial, path)
}
