//! Launch-local ordering of hook and failed-turn reports. `/clear` names only the new
//! thread, so remember the last reported thread and announce its end on the existing channel.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use cockpit_provider_api::ConversationId;

pub const STATE_VARIABLE: &str = "COCKPIT_CODEX_REPORT_STATE";
const MAX_STATE_BYTES: u64 = 1024 * 1024;

/// A fresh file per launcher invocation, so old hooks cannot change a replacement launch.
pub struct LaunchReports(PathBuf);

impl LaunchReports {
    pub fn new(socket: &Path) -> Result<Self, String> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let path = socket.with_file_name(format!("codex-reports-{}-{nonce}", std::process::id()));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
        Ok(Self(path))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for LaunchReports {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Send a report, holding the launch's lock across a clear's end and successor reports.
/// Retired threads stay retired even when an old hook or failed-turn notification arrives
/// late. The ledger is adapter bookkeeping, never authority: the daemon checks every report.
pub fn send(
    cockpit: &OsStr,
    state: Option<&Path>,
    report: &[String],
    clears: bool,
) -> Result<(), String> {
    let Some(path) = state else {
        return run(cockpit, report);
    };
    let failed = |error| format!("cannot use {}: {error}", path.display());
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(failed)?;
    file.lock().map_err(failed)?;
    let mut ledger = String::new();
    Read::by_ref(&mut file)
        .take(MAX_STATE_BYTES + 1)
        .read_to_string(&mut ledger)
        .map_err(failed)?;
    if ledger.len() as u64 > MAX_STATE_BYTES
        || (!ledger.is_empty() && !ledger.ends_with('\n'))
        || ledger.lines().any(|id| ConversationId::new(id).is_err())
    {
        return Err(format!("{} is not a conversation ledger", path.display()));
    }
    let Some(thread) = report
        .windows(2)
        .find(|words| words[0] == "--conversation")
        .map(|words| words[1].as_str())
    else {
        return run(cockpit, report);
    };
    let current = ledger.lines().last();
    if current != Some(thread) && ledger.lines().any(|id| id == thread) {
        return Ok(());
    }
    if clears && let Some(previous) = current.filter(|previous| *previous != thread) {
        let ending = run(
            cockpit,
            &[
                "agent",
                "report",
                "--conversation",
                previous,
                "--conversation-ends",
            ]
            .map(str::to_owned),
        );
        match ending {
            Ok(()) => {
                // Retire the predecessor before sending the successor: if that report fails,
                // a late old hook must not consume the daemon's end announcement.
                append(&mut file, thread).map_err(failed)?;
                return run(cockpit, report);
            }
            Err(ended) => {
                // The previous successor may still be pending, or its acknowledgement lost.
                // Only the daemon can accept this next conversation.
                run(cockpit, report).map_err(|error| format!("{ended}; {error}"))?;
                append(&mut file, thread).map_err(failed)?;
                return Ok(());
            }
        }
    }
    run(cockpit, report)?;
    if current != Some(thread) {
        append(&mut file, thread).map_err(failed)?;
    }
    Ok(())
}

fn append(file: &mut File, thread: &str) -> std::io::Result<()> {
    file.seek(SeekFrom::End(0))?;
    writeln!(file, "{thread}")?;
    file.sync_data()
}

fn run(cockpit: &OsStr, report: &[String]) -> Result<(), String> {
    let output = Command::new(cockpit)
        .args(report)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("cannot run {}: {error}", cockpit.to_string_lossy()))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "`cockpit {}` failed ({}): {}",
            report.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}
