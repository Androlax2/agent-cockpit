//! What `torlek update` remembers between runs (#617): the owner's choices (automatic checks
//! on or off, the version skipped, the postponement) and the cache of the last check, in one
//! file in the profile's state directory. The rules for when to check and when to speak are
//! pure functions of this state and the clock, so tests drive them without either.

use std::fs::{self, DirBuilder};
use std::io::ErrorKind;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use serde_json::{Map, Value, json};

use super::release::{Installed, Offer, Version};

/// The owner's choice (2026-11-04): ask the release source and speak at most once a day.
pub(super) const CHECK_INTERVAL_SECONDS: i64 = 24 * 60 * 60;

/// "Later" postpones the notice for three days.
pub(super) const POSTPONE_SECONDS: i64 = 3 * 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct UpdateState {
    pub(super) automatic_checks: bool,
    /// Seconds since the epoch of the last check that ran, whether or not it succeeded, so
    /// a source that is down or rate limiting is asked once a day, not at every command.
    pub(super) last_attempt_at: Option<i64>,
    /// The newest offer the last successful check found for this installation, if any.
    pub(super) latest: Option<Offer>,
    pub(super) skipped_version: Option<Version>,
    pub(super) postponed_until: Option<i64>,
    pub(super) last_notified_at: Option<i64>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            automatic_checks: true,
            last_attempt_at: None,
            latest: None,
            skipped_version: None,
            postponed_until: None,
            last_notified_at: None,
        }
    }
}

impl UpdateState {
    /// Whether a command that checks automatically should ask the release source now.
    pub(super) fn is_check_due(&self, now: i64) -> bool {
        self.automatic_checks && has_elapsed(self.last_attempt_at, now, CHECK_INTERVAL_SECONDS)
    }

    /// The offer an automatic notice may name now: newer than `installed`, not skipped, not
    /// postponed and not already announced within the last day.
    pub(super) fn offer_to_announce(&self, installed: &Installed, now: i64) -> Option<&Offer> {
        if !self.automatic_checks {
            return None;
        }
        let offer = self.known_offer(installed)?;
        let is_skipped = self.skipped_version.as_ref() == Some(&offer.version);
        let is_postponed = self.postponed_until.is_some_and(|until| now < until);
        let was_announced_today = !has_elapsed(self.last_notified_at, now, CHECK_INTERVAL_SECONDS);
        (!is_skipped && !is_postponed && !was_announced_today).then_some(offer)
    }

    /// The cached offer while it is still newer than what is installed: updating by hand
    /// between two checks must not leave a stale notice behind.
    pub(super) fn known_offer(&self, installed: &Installed) -> Option<&Offer> {
        self.latest
            .as_ref()
            .filter(|offer| offer.version > installed.version)
    }

    pub(super) fn postpone(&mut self, now: i64) {
        self.postponed_until = Some(now + POSTPONE_SECONDS);
    }

    pub(super) fn skip(&mut self, version: Version) {
        self.skipped_version = Some(version);
        self.postponed_until = None;
    }
}

/// True when `interval` has passed since `since`, or there is no `since`. A `since` in the
/// future (the clock was set back) counts as passed: it must not silence the notice until
/// the clock catches up.
fn has_elapsed(since: Option<i64>, now: i64, interval: i64) -> bool {
    since.is_none_or(|since| since > now || now - since >= interval)
}

/// The state file, or the defaults when none was written yet. A file that cannot be read as
/// state is an error naming it: the owner's choices are not silently reset.
pub(super) fn load(path: &Path) -> Result<UpdateState, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(UpdateState::default()),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    let value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("{}: not JSON: {error}", path.display()))?;
    from_value(&value).map_err(|problem| {
        format!(
            "{}: {problem}; delete the file to reset the update choices",
            path.display()
        )
    })
}

pub(super) fn save(path: &Path, state: &UpdateState) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| format!("{}: has no directory", path.display()))?;
    // 0700 because the daemon refuses a state directory any wider, and on a fresh install
    // this check can create it first (#961).
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    // Written beside the file and renamed over it, so a reader never sees half a file; the
    // process id keeps two commands running at once off each other's temporary file.
    let temporary = path.with_extension(format!("json.{}.new", std::process::id()));
    let text = serde_json::to_string_pretty(&to_value(state))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    fs::write(&temporary, format!("{text}\n"))
        .map_err(|error| format!("{}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("{}: {error}", path.display()))
}

fn to_value(state: &UpdateState) -> Value {
    let mut object = Map::new();
    object.insert("automatic_checks".to_owned(), json!(state.automatic_checks));
    object.insert("last_attempt_at".to_owned(), json!(state.last_attempt_at));
    object.insert("postponed_until".to_owned(), json!(state.postponed_until));
    object.insert("last_notified_at".to_owned(), json!(state.last_notified_at));
    object.insert(
        "skipped_version".to_owned(),
        json!(state.skipped_version.as_ref().map(Version::to_string)),
    );
    object.insert(
        "latest".to_owned(),
        match &state.latest {
            Some(offer) => json!({"version": offer.version.to_string(), "notes": offer.notes}),
            None => Value::Null,
        },
    );
    Value::Object(object)
}

fn from_value(value: &Value) -> Result<UpdateState, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "expected a JSON object".to_owned())?;
    let automatic_checks = match object.get("automatic_checks") {
        Some(Value::Bool(is_on)) => *is_on,
        None => true,
        Some(_) => return Err("\"automatic_checks\" is not true or false".to_owned()),
    };
    let latest = match object.get("latest") {
        None | Some(Value::Null) => None,
        Some(offer) => Some(offer_from_value(offer)?),
    };
    let skipped_version = match object.get("skipped_version") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(Version::parse(text)?),
        Some(_) => return Err("\"skipped_version\" is not text".to_owned()),
    };
    Ok(UpdateState {
        automatic_checks,
        last_attempt_at: seconds(object, "last_attempt_at")?,
        latest,
        skipped_version,
        postponed_until: seconds(object, "postponed_until")?,
        last_notified_at: seconds(object, "last_notified_at")?,
    })
}

fn offer_from_value(value: &Value) -> Result<Offer, String> {
    let version = value
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| "\"latest\" has no \"version\" text".to_owned())?;
    let notes = value
        .get("notes")
        .and_then(Value::as_str)
        .ok_or_else(|| "\"latest\" has no \"notes\" text".to_owned())?;
    Ok(Offer {
        version: Version::parse(version)?,
        notes: notes.to_owned(),
    })
}

fn seconds(object: &Map<String, Value>, name: &str) -> Result<Option<i64>, String> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| format!("{name:?} is not a whole number of seconds")),
    }
}
