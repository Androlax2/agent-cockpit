//! The phone page (claude-remote): the Claude sessions of this computer, from outside the home,
//! over Tailscale only (`tailscale serve` publishes the server bound to 127.0.0.1).
//!
//! A phone gets in by pairing: `claude-remote pair` shows a one-time link (a QR code), valid 10
//! minutes; opening it gives the phone's browser a long secret in a cookie. Only the SHA-256 of
//! codes and secrets is kept, in ~/.local/state/claude-remote (0600; greywall hides it from every
//! sandbox). `claude-remote phones` lists them; `claude-remote unpair [id]` forgets one, or all.
//!
//! From the phone: the session cards, a session's last screen, a message (queued while it works,
//! as the column's m), an answer to a permission dialog, and Run or Decline for an approval that is
//! asking (claude-run-approve shows the same command from `<id>.asking` and takes the answer from
//! `<id>.remote`). Every action is an audit event (component "remote").

use crate::{approval, audit, paths, prompts, util};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Mutex;
use std::process::{Command, Stdio};

const PAIRING_VALID_S: i64 = 600;
const MESSAGE_MAX: usize = 4000;
const LAST_SEEN_STEP_S: i64 = 60;

pub fn state_dir() -> PathBuf {
    std::env::var("CLAUDE_REMOTE_DIR").map(PathBuf::from).unwrap_or_else(|_| paths::state_home().join("claude-remote"))
}

fn write_private(name: &str, value: &Value) -> std::io::Result<()> {
    let folder = state_dir();
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder)?;
    let temporary = folder.join(format!("{name}.tmp"));
    let mut file = OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&temporary)?;
    writeln!(file, "{value}")?;
    fs::rename(temporary, folder.join(name))
}

fn read_private(name: &str) -> Value {
    fs::read_to_string(state_dir().join(name)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(json!({}))
}

/// A new one-time pairing code for a phone called `name` (the previous code no longer works).
pub fn new_pairing_code(name: &str) -> std::io::Result<String> {
    let code = util::token_hex(16);
    write_private("pairing.json", &json!({"sha256": audit::sha256_hex(code.as_bytes()), "until": util::now_secs() + PAIRING_VALID_S, "name": name}))?;
    audit::record("remote", "remote.pairing_started", "ok", json!({"name": name}));
    Ok(code)
}

/// devices.json is read, changed and written back by several workers: one at a time.
static DEVICES: Mutex<()> = Mutex::new(());

fn devices() -> Vec<Value> {
    read_private("devices.json").get("devices").and_then(Value::as_array).cloned().unwrap_or_default()
}

fn write_devices(devices: Vec<Value>) -> std::io::Result<()> {
    write_private("devices.json", &json!({"devices": devices}))
}

/// The short id a phone is shown and forgotten by: the start of its secret's hash.
fn device_id(device: &Value) -> String {
    text(device, "sha256").chars().take(8).collect()
}

/// The phone's secret for a pairing code still valid, once: None for a wrong, used or old code.
pub fn redeem(code: &str) -> Option<String> {
    let _guard = DEVICES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let pairing = read_private("pairing.json");
    let is_valid = pairing.get("sha256").and_then(Value::as_str) == Some(audit::sha256_hex(code.as_bytes()).as_str())
        && pairing.get("until").and_then(Value::as_i64).is_some_and(|until| util::now_secs() <= until);
    if !is_valid {
        audit::record("remote", "remote.paired", "refused", json!({"reason": "a wrong, used or expired pairing code"}));
        return None;
    }
    let _ = fs::remove_file(state_dir().join("pairing.json"));
    let secret = util::token_hex(32);
    let mut known = devices();
    let name = Some(text(&pairing, "name")).filter(|name| !name.is_empty()).unwrap_or_else(|| format!("Phone {}", known.len() + 1));
    let now = util::now_secs();
    known.push(json!({"sha256": audit::sha256_hex(secret.as_bytes()), "name": name, "paired_at": now, "last_seen": now}));
    write_devices(known).ok()?;
    audit::record("remote", "remote.paired", "ok", json!({"name": name}));
    Some(secret)
}

/// The paired phone a secret belongs to (its id), its last visit noted once a minute at most.
pub fn paired_phone(secret: &str) -> Option<String> {
    if secret.len() < 32 {
        return None;
    }
    let hash = audit::sha256_hex(secret.as_bytes());
    let _guard = DEVICES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut known = devices();
    let device = known.iter_mut().find(|device| device.get("sha256").and_then(Value::as_str) == Some(hash.as_str()))?;
    let id = device_id(device);
    let now = util::now_secs();
    if device.get("last_seen").and_then(Value::as_i64).is_none_or(|seen| now - seen >= LAST_SEEN_STEP_S) {
        device["last_seen"] = json!(now);
        let _ = write_devices(known);
    }
    Some(id)
}

/// The paired phones: id, name, when paired, when last seen. No secret, no hash.
pub fn phones() -> Vec<Value> {
    devices().iter().map(|device| json!({
        "id": device_id(device), "name": text(device, "name"), "paired_at": device.get("paired_at"), "last_seen": device.get("last_seen"),
    })).collect()
}

/// One phone forgotten, by its id.
pub fn unpair(id: &str) -> Result<String, String> {
    let _guard = DEVICES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let (forgotten, kept): (Vec<Value>, Vec<Value>) = devices().into_iter().partition(|device| !id.is_empty() && device_id(device) == id);
    let Some(phone) = forgotten.first() else { return Err(format!("no paired phone {id}")) };
    let name = text(phone, "name");
    write_devices(kept).map_err(|error| error.to_string())?;
    audit::record("remote", "remote.unpaired", "ok", json!({"id": id, "name": name}));
    Ok(name)
}

/// Every phone forgotten.
pub fn unpair_all() -> std::io::Result<()> {
    let _guard = DEVICES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    write_devices(Vec::new())?;
    let _ = fs::remove_file(state_dir().join("pairing.json"));
    audit::record("remote", "remote.unpaired", "ok", json!({"id": "all"}));
    Ok(())
}

fn snapshot_sessions() -> Vec<Value> {
    util::read_json(&paths::snapshot()).and_then(|snapshot| snapshot.get("sessions").and_then(Value::as_array).cloned()).unwrap_or_default()
}

fn session_row(id: &str) -> Option<Value> {
    snapshot_sessions().into_iter().find(|row| row.get("session").and_then(Value::as_str) == Some(id))
}

fn text(row: &Value, key: &str) -> String {
    row.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The approvals asking now: the ones whose menu is up (`<id>.asking`), with what they show.
pub fn asking_approvals() -> Vec<Value> {
    let Ok(entries) = fs::read_dir(approval::log_dir()) else { return Vec::new() };
    let mut asking: Vec<Value> = entries.flatten().map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "asking"))
        .filter_map(|path| util::read_json(&path))
        .filter(|approval| approval.get("id").and_then(Value::as_str).is_some_and(approval::valid_id))
        .collect();
    asking.sort_by_key(|approval| text(approval, "id"));
    asking
}

/// What the page shows: the session cards and the approvals asking.
pub fn state() -> Value {
    let cards: Vec<Value> = snapshot_sessions().iter().map(|row| json!({
        "session": text(row, "session"), "project": text(row, "project"), "state": text(row, "state"), "task": text(row, "task"),
        "request": text(row, "request"), "detail": text(row, "detail"), "since": row.get("since"),
        "queued": prompts::count(&text(row, "session")),
    })).collect();
    json!({"sessions": cards, "approvals": asking_approvals()})
}

fn session_pane(id: &str) -> Result<String, String> {
    let row = session_row(id).ok_or("no such session")?;
    let pane = text(&row, "tmux_pane");
    if !regex::Regex::new(r"^%[0-9]{1,10}$").unwrap().is_match(&pane) {
        return Err("this session has no tmux pane".into());
    }
    Ok(pane)
}

fn pane_lines(pane: &str, lines: u32) -> Result<String, String> {
    util::output_of("tmux", &["capture-pane", "-p", "-t", pane, "-S", &format!("-{lines}")]).ok_or_else(|| "tmux could not read its pane".into())
}

/// The state the pane's last lines show, by the daemon's own reading (osd classify).
fn pane_state(tail: &str) -> String {
    let child = Command::new("osd").arg("classify").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let Ok(mut child) = child else { return String::new() };
    if let Some(mut input) = child.stdin.take() {
        let _ = input.write_all(tail.as_bytes());
    }
    child.wait_with_output().map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string()).unwrap_or_default()
}

fn press(pane: &str, key: &str) -> Result<(), String> {
    let pressed = Command::new("tmux").args(["send-keys", "-t", pane, key]).stdin(Stdio::null()).status();
    if pressed.is_ok_and(|status| status.success()) { Ok(()) } else { Err("tmux could not press the key".into()) }
}

/// The last lines a session's pane shows, as text.
pub fn screen(id: &str) -> Result<String, String> {
    Ok(approval::strip_ansi(&pane_lines(&session_pane(id)?, 80)?))
}

/// A message for a session: said at its prompt now, or queued for the end of its turn while it
/// works (as the column's m).
pub fn message(id: &str, message: &str) -> Result<String, String> {
    let row = session_row(id).ok_or("no such session")?;
    let message = message.trim();
    if message.is_empty() || message.chars().count() > MESSAGE_MAX || message.chars().any(char::is_control) {
        return Err(format!("a message is one line of at most {MESSAGE_MAX} characters"));
    }
    let outcome = if text(&row, "state") == "working" {
        prompts::add(id, message)?;
        "queued for the end of its turn".to_string()
    } else {
        let error = util::manager(&["say", id, message]);
        if !error.is_empty() {
            return Err(error);
        }
        "sent".to_string()
    };
    audit::record("remote", "remote.message", "ok", json!({"session": id, "message": audit::fingerprint(message), "outcome": outcome}));
    Ok(outcome)
}

/// Yes or no to the permission dialog a session shows. The pane is read again right before: Yes
/// presses Enter only while the dialog is open with its cursor on "1. Yes"; No presses Escape,
/// Claude's own "no".
pub fn answer(id: &str, is_yes: bool) -> Result<String, String> {
    let pane = session_pane(id)?;
    let tail = pane_lines(&pane, 20)?;
    if pane_state(&tail) != "needs-you" {
        return Err("this session asks nothing now".into());
    }
    if is_yes && !regex::Regex::new(r"❯\s*1\.\s*Yes").unwrap().is_match(&tail) {
        return Err("the dialog's cursor is not on \"1. Yes\": answer it in the session".into());
    }
    press(&pane, if is_yes { "Enter" } else { "Escape" })?;
    audit::record("remote", "remote.answer", if is_yes { "yes" } else { "no" }, json!({"session": id}));
    Ok(if is_yes { "approved".into() } else { "declined".into() })
}

/// Run or Decline for an approval that is asking now: its menu takes the answer (claude-run-approve
/// polls `<id>.remote`). The command run is the one the page showed (`<id>.asking`).
pub fn decide(id: &str, is_run: bool, shown_command: &str) -> Result<String, String> {
    if !approval::valid_id(id) {
        return Err("not a request id".into());
    }
    let asking = util::read_json(&approval::remote_asking(id)).ok_or("this approval asks nothing now")?;
    let command = text(&asking, "command");
    if command != shown_command {
        return Err("the command changed since the page showed it: look again".into());
    }
    if Command::new("tmux").args(["has-session", "-t", &format!("=claude-run-{id}")]).status().is_ok_and(|status| !status.success()) {
        return Err("this approval ended".into());
    }
    fs::write(approval::remote_answer(id), if is_run { "y" } else { "n" }).map_err(|error| error.to_string())?;
    audit::record("remote", if is_run { "remote.run" } else { "remote.decline" }, "ok",
                  json!({"id": id, "project": text(&asking, "project"), "command": audit::fingerprint(&command)}));
    Ok(if is_run { "it runs".into() } else { "declined".into() })
}
