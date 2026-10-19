//! The phone page (claude-remote): the Claude sessions of this computer, from outside the home,
//! over Tailscale only (`tailscale serve` publishes the server bound to 127.0.0.1).
//!
//! A phone gets in by pairing: `claude-remote pair` shows a one-time link (a QR code), valid 10
//! minutes; opening it gives the phone's browser a long secret in a cookie. Only the SHA-256 of
//! codes and secrets is kept, in ~/.local/state/claude-remote (0600; greywall hides it from every
//! sandbox). `claude-remote phones` lists them; `claude-remote unpair [id]` forgets one, or all.
//!
//! From the phone: the session cards, a session's last screen (in colour), a message (queued while
//! it works, as the column's m), an answer to a permission dialog, Escape for a session that works,
//! Remote Control on or off, and Run or Decline for an approval that is
//! asking (claude-run-approve shows the same command from `<id>.asking` and takes the answer from
//! `<id>.remote`). Every action is an audit event (component "remote").

use crate::{approval, audit, paths, prompts, push, util};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Mutex;
use std::process::{Command, Stdio};

pub const DEFAULT_ADDRESS: &str = "127.0.0.1:7380";
const PAIRING_VALID_S: i64 = 600;
const MESSAGE_MAX: usize = 4000;
const LAST_SEEN_STEP_S: i64 = 60;

/// Where the server listens: CLAUDE_REMOTE_ADDRESS, or 127.0.0.1:7380.
pub fn address() -> String {
    std::env::var("CLAUDE_REMOTE_ADDRESS").unwrap_or_else(|_| DEFAULT_ADDRESS.into())
}

pub fn state_dir() -> PathBuf {
    std::env::var("CLAUDE_REMOTE_DIR").map(PathBuf::from).unwrap_or_else(|_| paths::state_home().join("claude-remote"))
}

pub(crate) fn write_private(name: &str, value: &Value) -> std::io::Result<()> {
    let folder = state_dir();
    fs::DirBuilder::new().recursive(true).mode(0o700).create(&folder)?;
    let temporary = folder.join(format!("{name}.tmp"));
    let mut file = OpenOptions::new().create(true).write(true).truncate(true).mode(0o600).open(&temporary)?;
    writeln!(file, "{value}")?;
    fs::rename(temporary, folder.join(name))
}

pub(crate) fn read_private(name: &str) -> Value {
    fs::read_to_string(state_dir().join(name)).ok().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(json!({}))
}

/// A new one-time pairing code for a phone called `name` (the previous code no longer works).
pub fn new_pairing_code(name: &str) -> std::io::Result<String> {
    let code = util::token_hex(16);
    write_private("pairing.json", &json!({"sha256": audit::sha256_hex(code.as_bytes()), "until": util::now_secs() + PAIRING_VALID_S, "name": name}))?;
    audit::record("remote", "remote.pairing_started", "ok", json!({"name": name}));
    Ok(code)
}

/// The tailnet name of this computer (tailscale status), for the pairing link.
fn tailnet_name() -> Option<String> {
    let status: Value = serde_json::from_str(&util::output_of("tailscale", &["status", "--json"])?).ok()?;
    status.get("Self")?.get("DNSName")?.as_str().map(|name| name.trim_end_matches('.').to_string()).filter(|name| !name.is_empty())
}

/// A new one-time pairing link for a phone called `name` ("" lets the phone name itself).
pub fn pairing_link(name: &str) -> Result<String, String> {
    if name.chars().count() > 32 || name.chars().any(char::is_control) {
        return Err("a phone's name is one line of at most 32 characters".into());
    }
    let host = tailnet_name().ok_or("tailscale status gives no name for this computer: is Tailscale up?")?;
    let code = new_pairing_code(name).map_err(|error| format!("cannot keep the pairing code: {error}"))?;
    Ok(format!("https://{host}/pair/{code}"))
}

/// The QR code of `text` as rows of modules (true is dark), quiet zone included, from qrencode.
pub fn qr_modules(text: &str) -> Option<Vec<Vec<bool>>> {
    let drawn = util::output_of("qrencode", &["-t", "ASCII", "-m", "2", text])?;
    let rows: Vec<Vec<bool>> = drawn.lines().filter(|line| !line.is_empty())
        .map(|line| line.as_bytes().chunks(2).map(|module| module[0] == b'#').collect()).collect();
    (!rows.is_empty()).then_some(rows)
}

/// A name for a phone paired without one, from its browser.
fn name_from_browser(user_agent: &str) -> Option<&'static str> {
    [("iPhone", "iPhone"), ("iPad", "iPad"), ("Android", "Android")].into_iter()
        .find(|(marker, _)| user_agent.contains(marker)).map(|(_, name)| name)
}

/// Whether the server answers on its address (GET /health), within half a second.
pub fn server_answers() -> bool {
    let Ok(address) = address().parse::<std::net::SocketAddr>() else { return false };
    let timeout = std::time::Duration::from_millis(500);
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&address, timeout) else { return false };
    let _ = stream.set_read_timeout(Some(timeout));
    if stream.write_all(b"GET /health HTTP/1.0\r\n\r\n").is_err() {
        return false;
    }
    let mut response = String::new();
    let _ = std::io::Read::read_to_string(&mut stream, &mut response);
    response.starts_with("HTTP/1.1 200") || response.starts_with("HTTP/1.0 200")
}

/// Whether `tailscale serve` publishes the server's port on the tailnet (None: Tailscale did not say).
pub fn is_published() -> Option<bool> {
    let port = address().rsplit(':').next()?.to_string();
    let serving = util::output_of("tailscale", &["serve", "status", "--json"])?;
    Some(serving.contains(&format!("127.0.0.1:{port}")) || serving.contains(&format!("localhost:{port}")))
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
pub fn redeem(code: &str, user_agent: &str) -> Option<String> {
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
    let name = Some(text(&pairing, "name")).filter(|name| !name.is_empty())
        .or_else(|| name_from_browser(user_agent).map(str::to_string))
        .unwrap_or_else(|| format!("Phone {}", known.len() + 1));
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

/// A one-time pairing code a paired phone asks for, for its Home Screen app: on iOS that app keeps
/// cookies of its own, so it pairs anew. The phone already holds everything it could reach.
pub fn pairing_code_from(phone: &str) -> Result<String, String> {
    let name = devices().iter().find(|device| device_id(device) == phone).map(|device| text(device, "name")).ok_or("no such phone")?;
    let home_screen_name: String = format!("{name} (Home Screen)").chars().take(32).collect();
    let code = new_pairing_code(&home_screen_name).map_err(|error| error.to_string())?;
    audit::record("remote", "remote.pairing_from_phone", "ok", json!({"phone": phone}));
    Ok(code)
}

/// A phone's push subscription kept (Some) or dropped (None).
pub fn set_push(phone: &str, subscription: Option<Value>) -> Result<(), String> {
    let _guard = DEVICES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut known = devices();
    let device = known.iter_mut().find(|device| device_id(device) == phone).ok_or("no such phone")?;
    match subscription {
        Some(subscription) => device["push"] = subscription,
        None => {
            device.as_object_mut().map(|fields| fields.remove("push"));
        }
    }
    write_devices(known).map_err(|error| error.to_string())?;
    audit::record("remote", "remote.push", if device_has_push(phone) { "on" } else { "off" }, json!({"phone": phone}));
    Ok(())
}

fn device_has_push(phone: &str) -> bool {
    devices().iter().any(|device| device_id(device) == phone && device.get("push").is_some())
}

/// The phones that take notifications: id and subscription.
pub fn push_targets() -> Vec<(String, Value)> {
    devices().iter().filter_map(|device| Some((device_id(device), device.get("push")?.clone()))).collect()
}

/// What deserves a notification now, each with a key that stays the same while it lasts: a
/// session that needs Theo (per request), an ask-to-run that asks. A key not seen on the previous
/// look is a new notification.
pub fn alerts(sessions: &[Value], approvals: &[Value]) -> Vec<(String, Value)> {
    let mut found = Vec::new();
    for session in sessions.iter().filter(|session| text(session, "state") == "needs-you") {
        let (id, request) = (text(session, "session"), Some(text(session, "detail")).filter(|detail| !detail.is_empty()).unwrap_or(text(session, "request")));
        found.push((format!("session {id} {request}"), json!({
            "title": format!("{} needs you", text(session, "project")), "body": shorten(&request), "session": id, "topic": id,
        })));
    }
    for asking in approvals {
        let id = text(asking, "id");
        // Characters that could hide what the command does show as �, never as what they do.
        let command: String = text(asking, "command").chars().map(|character| if approval::is_hiding(character) { '�' } else { character }).collect();
        found.push((format!("approval {id}"), json!({
            "title": format!("{} asks to run", text(asking, "project")), "body": shorten(&command), "topic": id,
        })));
    }
    found
}

fn shorten(text: &str) -> String {
    let short: String = text.chars().take(160).collect();
    if short.len() < text.len() { format!("{short}…") } else { short }
}

/// Everything that deserves a notification now (alerts over the snapshot and the approvals).
pub fn current_alerts() -> Vec<(String, Value)> {
    alerts(&snapshot_sessions(), &asking_approvals())
}

/// The VAPID subject: the page's own address, where the push service can reach whoever sends.
fn push_contact() -> String {
    tailnet_name().map(|host| format!("https://{host}")).unwrap_or_else(|| "https://localhost".into())
}

/// A notification to one phone, to check they reach it.
pub fn send_test(phone: &str) -> Result<String, String> {
    let (_, subscription) = push_targets().into_iter().find(|(id, _)| id == phone).ok_or("notifications are off on this phone")?;
    let test = json!({"title": "Claude sessions", "body": "Notifications reach this phone.", "topic": "test"});
    match push::send(&subscription, &test, "test", &push_contact()) {
        push::Delivery::Sent => Ok("sent: it shows in a few seconds".into()),
        push::Delivery::Gone => {
            let _ = set_push(phone, None);
            Err("the push service forgot this phone: turn notifications on again".into())
        }
        push::Delivery::Failed(error) => Err(error),
    }
}

/// The new alerts sent to every phone that takes notifications; a phone whose subscription is gone
/// stops taking them.
pub fn notify(new_alerts: &[(String, Value)]) {
    let contact = push_contact();
    for (phone, subscription) in push_targets() {
        for (_, notification) in new_alerts {
            let topic = notification.get("topic").and_then(Value::as_str).unwrap_or("");
            match push::send(&subscription, notification, topic, &contact) {
                push::Delivery::Sent => {}
                push::Delivery::Gone => {
                    let _ = set_push(&phone, None);
                    break;
                }
                push::Delivery::Failed(error) => util::log_event("WARN", &format!("claude-remote: no notification to phone {phone}: {error}")),
            }
        }
    }
}

/// When a phone was last seen, said shortly: "now", "12 min ago", "3 h ago", "4 days ago".
pub fn seen_ago(seconds: Option<i64>) -> String {
    let Some(seconds) = seconds else { return "never".into() };
    let elapsed = util::now_secs() - seconds;
    if elapsed < 120 {
        "now".into()
    } else if elapsed < 7200 {
        format!("{} min ago", elapsed / 60)
    } else if elapsed < 172_800 {
        format!("{} h ago", elapsed / 3600)
    } else {
        format!("{} days ago", elapsed / 86_400)
    }
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

/// The snapshot's "remote": true, or the Remote Control link (as sessions.rs reads it).
fn has_remote_control(row: &Value) -> bool {
    row.get("remote").is_some_and(|remote| remote.as_bool().unwrap_or(!remote.is_null()))
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
        "remote_control": has_remote_control(row),
        "agent": Some(text(row, "agent")).filter(|agent| !agent.is_empty()).unwrap_or_else(|| "claude".into()),
    })).collect();
    json!({"sessions": cards, "approvals": asking_approvals()})
}

/// A session's row when it runs Claude Code: its dialogs and keys are what these actions know
/// (as claude-sessions' require_claude_session).
fn require_claude(id: &str, what: &str) -> Result<Value, String> {
    let row = session_row(id).ok_or("no such session")?;
    if text(&row, "agent") == "opencode" {
        return Err(format!("{what} is Claude Code's; {} runs opencode: do it in its pane", text(&row, "project")));
    }
    Ok(row)
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

/// The last lines a session's pane shows, with its colours: of the terminal's escape sequences only
/// the colour and weight ones (SGR, `ESC [ … m`) are kept, the page draws them; every other one is
/// dropped, and so is any other control character but the line break and the tab.
pub fn screen(id: &str) -> Result<String, String> {
    let pane = session_pane(id)?;
    let shown = util::output_of("tmux", &["capture-pane", "-p", "-e", "-t", &pane, "-S", "-80"]).ok_or("tmux could not read its pane")?;
    Ok(colours_only(&shown))
}

pub fn colours_only(text: &str) -> String {
    let escape = regex::Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b.?").unwrap();
    let colour = regex::Regex::new(r"^\x1b\[[0-9;]*m$").unwrap();
    let kept = escape.replace_all(text, |found: &regex::Captures| {
        let sequence = &found[0];
        if colour.is_match(sequence) { sequence.to_string() } else { String::new() }
    });
    kept.chars().filter(|character| !character.is_control() || matches!(character, '\n' | '\t' | '\x1b')).collect()
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
    require_claude(id, "Yes and No")?;
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

/// Escape for a session that works: Claude stops its turn, as the key does in the session.
pub fn interrupt(id: &str) -> Result<String, String> {
    let row = require_claude(id, "Interrupt")?;
    if text(&row, "state") != "working" {
        return Err("this session is not working".into());
    }
    press(&session_pane(id)?, "Escape")?;
    audit::record("remote", "remote.interrupt", "ok", json!({"session": id}));
    Ok("interrupted".into())
}

/// Remote Control on or off for a session (as the column's : remote): claude-sessions restarts it.
pub fn toggle_remote_control(id: &str) -> Result<String, String> {
    let row = session_row(id).ok_or("no such session")?;
    let error = util::manager(&["remote", id]);
    if !error.is_empty() {
        return Err(error);
    }
    let was_on = has_remote_control(&row);
    audit::record("remote", "remote.remote_control", if was_on { "off" } else { "on" }, json!({"session": id}));
    Ok(if was_on { "Remote Control off".into() } else { "Remote Control on".into() })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn needs_you(request: &str) -> Value {
        json!({"session": "s1", "state": "needs-you", "project": "proj-a", "request": request})
    }

    #[test]
    fn a_session_that_needs_you_and_an_approval_that_asks_are_alerts() {
        let found = alerts(&[needs_you("Bash: make deploy"), json!({"session": "s2", "state": "waiting", "project": "proj-b"})],
                           &[json!({"id": "0123456789ab", "project": "proj-c", "command": "make test"})]);
        let shown: Vec<(&str, &str)> = found.iter().map(|(_, alert)| (alert["title"].as_str().unwrap(), alert["body"].as_str().unwrap())).collect();
        assert_eq!(shown, [("proj-a needs you", "Bash: make deploy"), ("proj-c asks to run", "make test")]);
    }

    #[test]
    fn a_new_request_in_the_same_session_is_a_new_alert() {
        let first = alerts(&[needs_you("Bash: make deploy")], &[]);
        assert_eq!(first[0].0, alerts(&[needs_you("Bash: make deploy")], &[])[0].0);
        assert_ne!(first[0].0, alerts(&[needs_you("Edit: src/main.rs")], &[])[0].0);
    }

    #[test]
    fn a_command_that_hides_characters_shows_them_as_replacement_marks() {
        let found = alerts(&[], &[json!({"id": "0123456789ab", "project": "p", "command": "echo hi\x1b[2K\rrm -rf ~"})]);
        assert_eq!(found[0].1["body"], "echo hi�[2K�rm -rf ~");
    }
}
