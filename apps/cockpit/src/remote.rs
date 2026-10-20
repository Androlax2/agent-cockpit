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

use crate::agents::{self, Feature};
use crate::{approval, audit, costs, paths, prompts, push, reviews, sessions, todo, usage, util};
use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;
use std::sync::Mutex;
use std::process::{Command, Stdio};

mod conversation;
pub use conversation::conversation;

pub const DEFAULT_ADDRESS: &str = "127.0.0.1:7380";
const PAIRING_VALID_S: i64 = 600;
const DIGITS_VALID_S: i64 = 120;
const DIGITS_TRIES: i64 = 5;
const MESSAGE_MAX: usize = 4000;
const LAST_SEEN_STEP_S: i64 = 60;
const PROJECTS_SHOWN: usize = 40;
const PULLS_FRESH_S: i64 = 300;

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
/// Also a 6-digit code for typing into the Home Screen app (scanned links open in Safari, which
/// keeps cookies apart from it): valid 2 minutes, and 5 wrong tries cancel it.
pub fn new_pairing_code(name: &str) -> std::io::Result<(String, String)> {
    let code = util::token_hex(16);
    let digits = format!("{:06}", u32::from_str_radix(&util::token_hex(4), 16).unwrap_or(0) % 1_000_000);
    let now = util::now_secs();
    write_private("pairing.json", &json!({
        "sha256": audit::sha256_hex(code.as_bytes()), "until": now + PAIRING_VALID_S, "name": name,
        "digits_sha256": audit::sha256_hex(digits.as_bytes()), "digits_until": now + DIGITS_VALID_S, "digits_tries_left": DIGITS_TRIES,
    }))?;
    audit::record("remote", "remote.pairing_started", "ok", json!({"name": name}));
    Ok((code, digits))
}

/// The tailnet name of this computer (tailscale status), for the pairing link.
fn tailnet_name() -> Option<String> {
    let status: Value = serde_json::from_str(&util::output_of("tailscale", &["status", "--json"])?).ok()?;
    status.get("Self")?.get("DNSName")?.as_str().map(|name| name.trim_end_matches('.').to_string()).filter(|name| !name.is_empty())
}

/// A new one-time pairing link for a phone called `name` ("" lets the phone name itself), and its
/// 6-digit code.
pub fn pairing_link(name: &str) -> Result<(String, String), String> {
    if name.chars().count() > 32 || name.chars().any(char::is_control) {
        return Err("a phone's name is one line of at most 32 characters".into());
    }
    let host = tailnet_name().ok_or("tailscale status gives no name for this computer: is Tailscale up?")?;
    let (code, digits) = new_pairing_code(name).map_err(|error| format!("cannot keep the pairing code: {error}"))?;
    Ok((format!("https://{host}/pair/{code}"), digits))
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
    admit(&pairing, user_agent)
}

/// The phone's secret for the 6-digit code, or why not. A wrong code costs a try; the last one
/// cancels the code (the link keeps working).
pub fn redeem_digits(digits: &str, user_agent: &str) -> Result<String, String> {
    let _guard = DEVICES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut pairing = read_private("pairing.json");
    let refuse = |reason: &str| {
        audit::record("remote", "remote.paired_by_digits", "refused", json!({"reason": reason}));
        Err(reason.to_string())
    };
    let (Some(expected), Some(until)) = (pairing.get("digits_sha256").and_then(Value::as_str), pairing.get("digits_until").and_then(Value::as_i64)) else {
        return refuse("no code is waiting: pair a phone on the computer first (S, Pair a phone)");
    };
    if util::now_secs() > until {
        return refuse("this code expired: pair a phone again on the computer");
    }
    if audit::sha256_hex(digits.as_bytes()) == expected {
        return admit(&pairing, user_agent).ok_or_else(|| "cannot keep this phone".to_string());
    }
    let tries_left = pairing.get("digits_tries_left").and_then(Value::as_i64).unwrap_or(0) - 1;
    if tries_left <= 0 {
        for field in ["digits_sha256", "digits_until", "digits_tries_left"] {
            pairing.as_object_mut().map(|fields| fields.remove(field));
        }
        let _ = write_private("pairing.json", &pairing);
        return refuse("too many wrong codes: this one is cancelled, pair a phone again on the computer");
    }
    pairing["digits_tries_left"] = json!(tries_left);
    let _ = write_private("pairing.json", &pairing);
    refuse(&format!("wrong code: {tries_left} tries left"))
}

/// A new paired phone for a pairing just redeemed (link or digits): both codes are used up.
/// The caller holds DEVICES.
fn admit(pairing: &Value, user_agent: &str) -> Option<String> {
    let _ = fs::remove_file(state_dir().join("pairing.json"));
    let secret = util::token_hex(32);
    let mut known = devices();
    let name = Some(text(pairing, "name")).filter(|name| !name.is_empty())
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
    let (code, _) = new_pairing_code(&home_screen_name).map_err(|error| error.to_string())?;
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

/// The sessions a phone is told about when their turn ends.
pub fn watched(phone: &str) -> Vec<String> {
    devices().iter().find(|device| device_id(device) == phone)
        .and_then(|device| device.get("watched")?.as_array().cloned()).unwrap_or_default()
        .iter().filter_map(|session| session.as_str().map(str::to_string)).collect()
}

/// A session's turn ends told to a phone (on), or no longer (off).
pub fn set_watch(phone: &str, session: &str, is_on: bool) -> Result<String, String> {
    let project = session_row(session).map(|row| text(&row, "project")).ok_or("no such session")?;
    let _guard = DEVICES.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut known = devices();
    let device = known.iter_mut().find(|device| device_id(device) == phone).ok_or("no such phone")?;
    let mut watching: Vec<String> = device.get("watched").and_then(Value::as_array).into_iter().flatten()
        .filter_map(|watched| watched.as_str().map(str::to_string)).filter(|watched| watched != session).collect();
    if is_on {
        watching.push(session.to_string());
    }
    // Sessions end: only the ones still running stay watched.
    let running: Vec<String> = snapshot_sessions().iter().map(|row| text(row, "session")).collect();
    watching.retain(|watched| running.contains(watched));
    device["watched"] = json!(watching);
    write_devices(known).map_err(|error| error.to_string())?;
    Ok(if is_on { format!("you hear when {project} finishes a turn") } else { format!("no more news of {project}'s turns") })
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
    // Only for the phones that watch the session (notify): its turn ended, and what it left.
    for session in sessions.iter().filter(|session| text(session, "state") == "waiting") {
        let id = text(session, "session");
        let since = session.get("since").map(Value::to_string).unwrap_or_default();
        let left = Some(text(session, "outcome")).filter(|outcome| !outcome.is_empty()).unwrap_or_else(|| text(session, "task"));
        found.push((format!("turn {id} {since}"), json!({
            "title": format!("{} finished", text(session, "project")), "body": shorten(&left), "session": id, "topic": id, "kind": "turn",
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

/// The notification of the lock handing the sessions to the phone, sent whatever they are doing:
/// how many need Theo (by project), wait for the next prompt, work, and ask to run.
pub fn handoff_alert(sessions: &[Value], approvals: &[Value]) -> Value {
    let in_state = |state: &str| sessions.iter().filter(|session| text(session, "state") == state).collect::<Vec<_>>();
    let needing = in_state("needs-you");
    let mut parts = Vec::new();
    if !needing.is_empty() {
        let projects: Vec<String> = needing.iter().map(|session| text(session, "project")).collect();
        parts.push(format!("{} need you ({})", needing.len(), projects.join(", ")));
    }
    for (state, said) in [("waiting", "wait for a prompt"), ("working", "working")] {
        let count = in_state(state).len();
        if count > 0 {
            parts.push(format!("{count} {said}"));
        }
    }
    if !approvals.is_empty() {
        parts.push(format!("{} ask to run", approvals.len()));
    }
    let body = if parts.is_empty() { "No session is open".to_string() } else { parts.join(" · ") };
    json!({"title": "Your sessions are on the phone", "body": shorten(&body), "topic": "handoff"})
}

fn shorten(text: &str) -> String {
    let short: String = text.chars().take(160).collect();
    if short.len() < text.len() { format!("{short}…") } else { short }
}

/// Everything that deserves a notification now (alerts over the snapshot and the approvals).
pub fn current_alerts() -> Vec<(String, Value)> {
    alerts(&snapshot_sessions(), &asking_approvals())
}

/// Whether a phone hears of an alert: everything while Theo is away (the lock screen's away on);
/// at the desk only the turn ends of the sessions it watches, which it asked for by name.
fn is_for_phone(notification: &Value, is_away: bool, watching: &[String]) -> bool {
    if is_away {
        return true;
    }
    let session = notification.get("session").and_then(Value::as_str).unwrap_or("");
    notification.get("kind").and_then(Value::as_str) == Some("turn") && watching.iter().any(|watched| watched == session)
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
    let is_away = paths::phone_away().exists();
    for (phone, subscription) in push_targets() {
        let watching = watched(&phone);
        for (_, notification) in new_alerts {
            if !is_for_phone(notification, is_away, &watching) {
                continue;
            }
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

pub fn snapshot_sessions() -> Vec<Value> {
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

fn approval_has_ended(id: &str) -> bool {
    Command::new("tmux").args(["has-session", "-t", &format!("=claude-run-{id}")])
        .status().is_ok_and(|status| !status.success())
}

/// The approvals asking now: the ones whose menu is up (`<id>.asking`), with what they show.
pub fn asking_approvals() -> Vec<Value> {
    let Ok(entries) = fs::read_dir(approval::log_dir()) else { return Vec::new() };
    let mut asking: Vec<Value> = entries.flatten().map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "asking"))
        .filter_map(|path| util::read_json(&path))
        .filter(|approval| approval.get("id").and_then(Value::as_str).is_some_and(approval::valid_id))
        .filter(|approval| !approval_has_ended(&text(approval, "id")))
        .collect();
    asking.sort_by_key(|approval| text(approval, "id"));
    asking
}

/// What the page shows: the session cards and the approvals asking.
pub fn state() -> Value {
    let cards: Vec<Value> = snapshot_sessions().iter().map(|row| json!({
        "session": text(row, "session"), "project": text(row, "project"), "state": text(row, "state"), "task": text(row, "task"),
        "request": text(row, "request"), "detail": text(row, "detail"), "since": row.get("since"), "outcome": text(row, "outcome"),
        "queued": prompts::count(&text(row, "session")),
        "needs_input": row.get("needs_input").and_then(Value::as_bool).unwrap_or(false),
        "remote_control": has_remote_control(row),
        "agent": Some(text(row, "agent")).filter(|agent| !agent.is_empty()).unwrap_or_else(|| agents::DEFAULT.into()),
        "features": agents::get(&text(row, "agent")).feature_names(),
    })).collect();
    json!({"sessions": cards, "approvals": asking_approvals()})
}

/// A session's row when its agent has `feature` (as claude-sessions' require_feature).
fn require_feature(id: &str, feature: Feature, what: &str) -> Result<Value, String> {
    let row = session_row(id).ok_or("no such session")?;
    let agent = agents::get(&text(&row, "agent"));
    if !agent.can(feature) {
        return Err(format!("{what} is not {}'s to do; {} runs {}: do it in its pane", agent.label, text(&row, "project"), agent.name));
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
    require_feature(id, Feature::Dialogs, "Yes and No")?;
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

/// The keys that stop a working session's turn (its agent's interrupt_keys), as in its pane.
pub fn interrupt(id: &str) -> Result<String, String> {
    let row = require_feature(id, Feature::Interrupt, "Interrupt")?;
    if text(&row, "state") != "working" {
        return Err("this session is not working".into());
    }
    let pane = session_pane(id)?;
    let keys = agents::get(&text(&row, "agent")).interrupt_keys;
    let pressed = Command::new("tmux").args(["send-keys", "-t", &pane]).args(keys).stdin(Stdio::null()).status();
    if !pressed.is_ok_and(|status| status.success()) {
        return Err("tmux could not press the keys".into());
    }
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

// ---- Work from the phone: new sessions, the todo list, pull requests, usage ----

/// The agents a new session can run (the column's agent pick): name and label.
pub fn agent_choices() -> Vec<Value> {
    agents::AGENTS.iter().map(|agent| json!({"name": agent.name, "label": agent.label})).collect()
}

/// The projects a session can start in (the column's a picker): path and name.
pub fn projects() -> Vec<Value> {
    sessions::project_choices().into_iter().take(PROJECTS_SHOWN)
        .map(|path| json!({"name": util::project_name(&path), "path": path})).collect()
}

fn known_project(path: &str) -> Result<String, String> {
    sessions::project_choices().into_iter().find(|known| known == path).ok_or_else(|| "not one of the projects".into())
}

fn one_line(text: &str, what: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MESSAGE_MAX || text.chars().any(char::is_control) {
        return Err(format!("{what} is one line of at most {MESSAGE_MAX} characters"));
    }
    Ok(text.to_string())
}

/// One of the agents (agents::AGENTS); anything else, an absent one included, is refused.
fn known_agent(agent: &str) -> Result<&'static str, String> {
    agents::find(agent).map(|known| known.name).ok_or_else(|| format!("which agent? one of {}", agents::names().join(", ")))
}

/// A new session in a project, out of sight, with its first message: in its own clone on a
/// phone-<time> branch (as the column's +), or in the project's folder itself.
pub fn start_session(project: &str, prompt: &str, in_clone: bool, agent: &str) -> Result<String, String> {
    let directory = known_project(project)?;
    let prompt = one_line(prompt, "the first message")?;
    let agent = known_agent(agent)?;
    let branch = format!("phone-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let arguments: Vec<&str> = if in_clone { vec!["--agent", agent, "parallel", &directory, &branch, &prompt] } else { vec!["--agent", agent, "background", &directory, &prompt] };
    let error = util::manager(&arguments);
    if !error.is_empty() {
        return Err(error);
    }
    audit::record("remote", "remote.session_started", "ok", json!({"project": directory, "clone": in_clone, "agent": agent, "prompt": audit::fingerprint(&prompt)}));
    Ok(if in_clone { format!("starting in a clone of {} (branch {branch})", util::project_name(&directory)) }
       else { format!("starting in {}", util::project_name(&directory)) })
}

/// The items to do and in progress, highest priority first (the Todo panel's order).
pub fn todos() -> Vec<Value> {
    let list = todo::TodoList::load();
    let mut open: Vec<todo::Entry> = list.items.iter().filter(|item| matches!(item.status().as_str(), "todo" | "doing"))
        .map(|item| todo::Entry::Typed(item.clone())).collect();
    open.sort_by(todo::todo_order);
    open.iter().filter_map(|entry| match entry {
        todo::Entry::Typed(item) => Some(json!({
            "id": item.id(), "project": item.project(), "text": item.text(), "notes": item.notes(),
            "status": item.status(), "priority": item.priority(),
        })),
        todo::Entry::Issue { .. } => None,
    }).collect()
}

/// A change for the todo list, through the queue the column applies (claude-todo's), so the
/// column stays the only writer of todo.json. `started_in` is the project's folder.
fn queue_todo_update(update: Value) -> Result<(), String> {
    let path = paths::todo_updates();
    let mut file = OpenOptions::new().create(true).append(true).mode(0o600).open(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    writeln!(file, "{update}").map_err(|error| error.to_string())
}

/// A new item in a project's list (the column adds it within a second or two).
pub fn add_todo(project: &str, text: &str, notes: &str) -> Result<String, String> {
    let directory = known_project(project)?;
    let text = one_line(text, "an item")?;
    queue_todo_update(json!({"action": "add", "started_in": directory, "cwd": directory, "text": text,
                             "notes": todo::multiline(notes, MESSAGE_MAX), "session": "the phone"}))?;
    audit::record("remote", "remote.todo_added", "ok", json!({"project": directory, "text": audit::fingerprint(&text)}));
    Ok("added".into())
}

/// An item handed to a new session in its own clone (the Todo panel's N), then in progress.
pub fn start_todo(id: &str, agent: &str) -> Result<String, String> {
    let agent = known_agent(agent)?;
    let list = todo::TodoList::load();
    let item = list.items.iter().find(|item| item.id() == id).ok_or("no such item")?;
    if item.status() == "done" {
        return Err("this item is done".into());
    }
    let entry = todo::Entry::Typed(item.clone());
    let (branch, text) = todo::handoff_of(&entry);
    let directory = item.cwd();
    let error = util::manager(&["--agent", agent, "parallel", &directory, &branch, &text]);
    if !error.is_empty() {
        return Err(error);
    }
    queue_todo_update(json!({"action": "doing", "id": id, "started_in": directory}))?;
    audit::record("remote", "remote.todo_started", "ok", json!({"id": id, "project": directory, "agent": agent}));
    Ok(format!("starting a new session in a clone of {} (branch {branch})", util::project_name(&directory)))
}

/// The open pull requests of the repositories sessions run in (or the review requests), fetched
/// with gh at most every 5 minutes unless asked again.
pub fn pull_requests(is_refresh: bool) -> Value {
    static FETCHED: Mutex<Option<(i64, Value)>> = Mutex::new(None);
    let mut fetched = FETCHED.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((at, pulls)) = fetched.as_ref().filter(|(at, _)| !is_refresh && util::now_secs() - at < PULLS_FRESH_S) {
        return json!({"pulls": pulls, "fetched_at": at});
    }
    let repositories: Vec<String> = reviews::session_checkouts("").into_keys().collect();
    let (rows, error) = reviews::fetch_reviews(&repositories);
    let pulls = json!(rows.iter().map(|row| json!({
        "title": row.title, "url": row.url, "number": row.number, "repository": row.repository, "author": row.author,
        "updated": reviews::review_age(&row.updated), "draft": row.draft, "review": row.review, "ci": row.ci,
    })).collect::<Vec<_>>());
    if !error.is_empty() {
        return json!({"pulls": pulls, "error": error});
    }
    let now = util::now_secs();
    *fetched = Some((now, pulls.clone()));
    json!({"pulls": pulls, "fetched_at": now})
}

/// The plan's limits (claude.ai's usage page, as the column's cache keeps it), the cost estimate
/// and the time with Claude: today, this week, this month.
pub fn usage() -> Value {
    let windows: Vec<Value> = usage::windows().iter().map(|window| json!({
        "label": window.label, "percentage": window.percentage, "resets": usage::resets_in(window), "note": window.note,
    })).collect();
    let checked_at = fs::metadata(paths::usage_cache()).and_then(|metadata| metadata.modified()).ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok()).map(|age| age.as_secs());
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let archived = rusqlite::Connection::open_with_flags(paths::usage_archive(), flags).and_then(|archive| {
        Ok((costs::archived_costs(&archive)?.0, costs::time_with_claude(&archive, &costs::period_starts())?))
    });
    let (cost, time) = match archived {
        Ok((days, time)) => {
            let totals = costs::totals_of(&days);
            (json!({"today": totals.today, "week": totals.week, "month": totals.month}), json!({"today": time[0], "week": time[1], "month": time[2]}))
        }
        Err(error) => (json!({"error": error.to_string()}), json!(null)),
    };
    json!({"plan": usage::plan(), "windows": windows, "checked_at": checked_at, "cost": cost, "time": time})
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
    if approval_has_ended(id) {
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
        assert_eq!(shown, [("proj-a needs you", "Bash: make deploy"), ("proj-b finished", ""), ("proj-c asks to run", "make test")]);
    }

    #[test]
    fn a_turn_end_says_what_the_turn_left_and_each_turn_is_a_new_alert() {
        let waiting = |since: i64| json!({"session": "s1", "state": "waiting", "project": "proj-a", "since": since, "outcome": "2 commits, tests pass"});
        let first = alerts(&[waiting(100)], &[]);
        assert_eq!((first[0].1["title"].as_str(), first[0].1["body"].as_str(), first[0].1["kind"].as_str()),
                   (Some("proj-a finished"), Some("2 commits, tests pass"), Some("turn")));
        assert_ne!(first[0].0, alerts(&[waiting(200)], &[])[0].0);
    }

    #[test]
    fn away_every_alert_reaches_the_phone_and_at_the_desk_only_a_watched_sessions_turn_end() {
        let needs = json!({"session": "s1", "title": "proj-a needs you"});
        let turn = |session: &str| json!({"session": session, "kind": "turn"});
        let watching = vec!["s1".to_string()];
        assert!(is_for_phone(&needs, true, &[]) && is_for_phone(&turn("s2"), true, &[]));
        assert!(!is_for_phone(&needs, false, &watching));
        assert!(is_for_phone(&turn("s1"), false, &watching));
        assert!(!is_for_phone(&turn("s2"), false, &watching));
    }

    #[test]
    fn the_handoff_counts_the_sessions_by_what_they_do_and_names_those_that_need_you() {
        let sessions = [needs_you("Bash: make deploy"), json!({"session": "s2", "state": "waiting", "project": "proj-b"}),
                        json!({"session": "s3", "state": "working", "project": "proj-c"}), json!({"session": "s4", "state": "working", "project": "proj-d"})];
        let handoff = handoff_alert(&sessions, &[json!({"id": "0123456789ab", "project": "proj-c", "command": "make test"})]);
        assert_eq!(handoff["body"], "1 need you (proj-a) · 1 wait for a prompt · 2 working · 1 ask to run");
    }

    #[test]
    fn the_handoff_is_sent_even_when_no_session_is_open() {
        assert_eq!(handoff_alert(&[], &[])["body"], "No session is open");
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
