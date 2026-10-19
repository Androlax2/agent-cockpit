//! The audit log (security audit 2026-10-19, M6): what was asked of Theo, what he approved and ran,
//! what was refused, from which sandbox, kept so that an incident can be traced afterwards.
//!
//! One JSON object per line in ~/.local/state/claude-audit/audit-YYYY-MM-DD.jsonl (0600; greywall
//! hides the folder from every sandbox), copied to journald (`journalctl -t claude-audit`). Each line
//! carries `seq` and `prev`, the SHA-256 of the line before it: a line removed or changed afterwards
//! breaks the chain, and `claude-audit verify` names where. Writers (the daemon, the column, the
//! approval and guide windows, claude-sessions through `claude-audit record`) take a lock on the
//! folder for each line. A text a sandbox chose (a command, a message, a title) goes in only as
//! `{sha256, len}` (fingerprint); short labels it chose (its session id, a folder) go in whole,
//! named claimed_* so they are never mistaken for what the daemon verified. An approved command is
//! kept whole in runs/<id>.json beside, 0600.
//! Files are gzipped after 7 days and deleted after 90. CLAUDE_AUDIT_DIR moves the folder and
//! CLAUDE_AUDIT_JOURNAL=off stops the journald copy (tests).

use crate::paths;
use chrono::{Local, NaiveDate};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const GZIP_AFTER_DAYS: i64 = 7;
const KEEP_DAYS: i64 = 90;

pub fn dir() -> PathBuf {
    std::env::var("CLAUDE_AUDIT_DIR").map(PathBuf::from).unwrap_or_else(|_| paths::state_home().join("claude-audit"))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A text from a sandbox, as the log keeps it: its hash and length, never the text itself.
pub fn fingerprint(text: &str) -> Value {
    json!({"sha256": sha256_hex(text.as_bytes()), "len": text.len()})
}

/// One event. `fields` is an object; `component`, `event` and `outcome` name it ("daemon",
/// "run.held", "ok"). Never fails: an audit write that cannot happen is said in the debug log.
pub fn record(component: &str, event: &str, outcome: &str, fields: Value) {
    if let Err(error) = append(component, event, outcome, fields) {
        crate::util::log_as(component, "ERROR", &format!("audit event {event} not written: {error}"));
    }
}

fn append(component: &str, event: &str, outcome: &str, fields: Value) -> std::io::Result<()> {
    let folder = dir();
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder)?;
    let lock = OpenOptions::new().create(true).write(true).truncate(false).mode(0o600).open(folder.join(".lock"))?;
    lock.lock()?;
    let (previous_seq, previous_hash) = read_head(&folder);
    let mut line = Map::new();
    line.insert("ts".into(), json!(Local::now().format("%Y-%m-%dT%H:%M:%S%.3f%:z").to_string()));
    line.insert("seq".into(), json!(previous_seq + 1));
    line.insert("prev".into(), json!(previous_hash));
    line.insert("component".into(), json!(component));
    line.insert("event".into(), json!(event));
    line.insert("outcome".into(), json!(outcome));
    if let Value::Object(fields) = fields {
        for (key, value) in fields {
            line.entry(key).or_insert(value);
        }
    }
    let text = Value::Object(line).to_string();
    let today = folder.join(format!("audit-{}.jsonl", Local::now().format("%Y-%m-%d")));
    let is_new_day = !today.exists();
    let mut file = OpenOptions::new().create(true).append(true).mode(0o600).open(&today)?;
    writeln!(file, "{text}")?;
    write_head(&folder, previous_seq + 1, &sha256_hex(text.as_bytes()))?;
    drop(lock);
    copy_to_journal(&text);
    if is_new_day {
        age_old_files(&folder);
    }
    Ok(())
}

/// The last line's seq and hash, from the `head` file; 0 and "" before the first line.
fn read_head(folder: &Path) -> (u64, String) {
    let text = fs::read_to_string(folder.join("head")).unwrap_or_default();
    let mut parts = text.split_whitespace();
    let seq = parts.next().and_then(|seq| seq.parse().ok()).unwrap_or(0);
    (seq, parts.next().unwrap_or("").to_string())
}

fn write_head(folder: &Path, seq: u64, hash: &str) -> std::io::Result<()> {
    let temporary = folder.join("head.tmp");
    fs::write(&temporary, format!("{seq} {hash}\n"))?;
    fs::rename(temporary, folder.join("head"))
}

fn copy_to_journal(text: &str) {
    if std::env::var("CLAUDE_AUDIT_JOURNAL").is_ok_and(|setting| setting == "off") {
        return;
    }
    let Ok(mut child) = Command::new("systemd-cat").args(["-t", "claude-audit"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
        return;
    };
    if let Some(mut input) = child.stdin.take() {
        let _ = writeln!(input, "{text}");
    }
    let _ = child.wait();
}

/// The day of an audit-YYYY-MM-DD.jsonl[.gz] file.
fn day_of(path: &Path) -> Option<NaiveDate> {
    let name = path.file_name()?.to_str()?;
    let day = name.strip_prefix("audit-")?.get(..10)?;
    NaiveDate::parse_from_str(day, "%Y-%m-%d").ok()
}

/// Once a day: older days gzipped, and what is past KEEP_DAYS (days and run sidecars) deleted.
fn age_old_files(folder: &Path) {
    let today = Local::now().date_naive();
    let Ok(entries) = fs::read_dir(folder) else { return };
    for path in entries.flatten().map(|entry| entry.path()) {
        let Some(day) = day_of(&path) else { continue };
        let age = (today - day).num_days();
        if age > KEEP_DAYS {
            let _ = fs::remove_file(&path);
        } else if age > GZIP_AFTER_DAYS && path.extension().is_some_and(|extension| extension == "jsonl") {
            let _ = Command::new("gzip").arg("-q").arg(&path).status();
        }
    }
    let Ok(runs) = fs::read_dir(folder.join("runs")) else { return };
    for path in runs.flatten().map(|entry| entry.path()) {
        let is_old = fs::metadata(&path).and_then(|metadata| metadata.modified()).ok()
            .and_then(|modified| modified.elapsed().ok()).is_some_and(|age| age.as_secs() as i64 > KEEP_DAYS * 86_400);
        if is_old {
            let _ = fs::remove_file(path);
        }
    }
}

/// An approved run's whole command, as asked and as run (Theo may have edited it), beside the log.
pub fn keep_command(id: &str, original: &str, final_command: &str) {
    let result = (|| -> std::io::Result<()> {
        let folder = dir().join("runs");
        fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder)?;
        let mut file = OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(folder.join(format!("{id}.json")))?;
        writeln!(file, "{}", json!({"id": id, "original_command": original, "final_command": final_command}))
    })();
    if let Err(error) = result {
        crate::util::log_as("run", "ERROR", &format!("audit: the command of {id} not kept: {error}"));
    }
}

/// Every line of every day, oldest first, gzipped days included.
fn all_lines(folder: &Path) -> std::io::Result<Vec<String>> {
    let mut days: Vec<PathBuf> = fs::read_dir(folder)?.flatten().map(|entry| entry.path()).filter(|path| day_of(path).is_some()).collect();
    days.sort();
    let mut lines = Vec::new();
    for day in days {
        let text = if day.extension().is_some_and(|extension| extension == "gz") {
            let output = Command::new("gzip").arg("-dc").arg(&day).output()?;
            String::from_utf8_lossy(&output.stdout).into_owned()
        } else {
            fs::read_to_string(&day)?
        };
        lines.extend(text.lines().map(str::to_string));
    }
    Ok(lines)
}

/// Whether the chain holds: Ok(lines checked), or the first line where it breaks and why. Days past
/// KEEP_DAYS are gone, so the first line kept may follow one that no longer exists.
pub fn verify() -> Result<usize, String> {
    let lines = all_lines(&dir()).map_err(|error| format!("cannot read {}: {error}", dir().display()))?;
    let mut previous: Option<(u64, String)> = None;
    for (index, line) in lines.iter().enumerate() {
        let event: Value = serde_json::from_str(line).map_err(|_| format!("line {} is not JSON", index + 1))?;
        let seq = event.get("seq").and_then(Value::as_u64).ok_or(format!("line {} has no seq", index + 1))?;
        let prev = event.get("prev").and_then(Value::as_str).unwrap_or("");
        if let Some((previous_seq, previous_hash)) = &previous {
            if seq != previous_seq + 1 {
                return Err(format!("seq {seq} follows seq {previous_seq}: lines are missing"));
            }
            if prev != previous_hash {
                return Err(format!("seq {seq} does not follow the line before it: that line was changed"));
            }
        }
        previous = Some((seq, sha256_hex(line.as_bytes())));
    }
    if let Some((seq, hash)) = previous {
        let (head_seq, head_hash) = read_head(&dir());
        if (head_seq, head_hash.as_str()) != (seq, hash.as_str()) {
            return Err(format!("the last line is seq {seq}, the head says {head_seq}: lines were removed at the end"));
        }
    }
    Ok(lines.len())
}
