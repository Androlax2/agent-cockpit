//! Idle session sleep: a session left at its prompt exits, its card stays as Asleep (one note in
//! paths::asleep_dir, named by the session id), and the conversation resumes on demand.

use crate::agents::Feature;
use crate::sessions::{self, Facts, Session};
use crate::{paths, settings, util};
use serde_json::Value;
use std::fmt;
use std::fs;
use std::path::Path;

/// A sleeping session that nobody woke for this long is forgotten: its conversation stays in the
/// agent's own history.
pub const EXPIRY_S: i64 = 7 * 86_400;
pub const DEFAULT_AFTER_MIN: i64 = 60;
pub const MIN_AFTER_MIN: i64 = 30;

#[derive(Clone, Debug, PartialEq)]
pub struct Asleep {
    pub session: String,
    pub agent: String,
    pub project: String,
    pub cwd: String,
    pub title: String,
    pub idle_since: i64,
    pub slept_at: i64,
    pub remote: bool,
    /// When the process that fell asleep started: the card keeps its place among its siblings.
    pub started: Option<i64>,
}

impl Asleep {
    /// The card that stands in for the session in the column: ready since it went idle, with no
    /// terminal target until it wakes. It is no process, so it never counts as started before a
    /// change (sessions::changed_since_start).
    pub fn card(&self) -> Session {
        Session {
            session: self.session.clone(),
            state: "waiting".into(),
            project: self.project.clone(),
            cwd: self.cwd.clone(),
            task: self.title.clone(),
            since: self.idle_since,
            started: self.started,
            agent: self.agent.clone(),
            asleep: Some(self.slept_at),
            facts: Facts {
                remote: self.remote,
                ..Facts::default()
            },
            ..Session::default()
        }
    }

    /// The session id is the note's file name, so a note cannot name a file outside its folder.
    fn from_note(session: String, note: &Value) -> Option<Asleep> {
        if let Some(permit) = note.get("team_resume")
            && (permit.as_str().is_none_or(str::is_empty)
                || note.get("session")?.as_str()? != session
                || note.get("slept_at")?.as_i64().is_none())
        {
            return None;
        }
        let text = |key: &str| {
            note.get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let seconds = |key: &str| {
            note.get(key)
                .and_then(Value::as_f64)
                .map(|seconds| seconds as i64)
        };
        Some(Asleep {
            session,
            agent: note
                .get("agent")
                .and_then(Value::as_str)
                .unwrap_or(crate::agents::DEFAULT)
                .into(),
            project: text("project"),
            cwd: text("cwd"),
            title: text("title"),
            idle_since: seconds("idle_since")?,
            slept_at: seconds("slept_at")?,
            remote: note.get("remote").and_then(Value::as_bool).unwrap_or(false),
            started: seconds("started"),
        })
    }
}

fn notes_in(dir: &Path) -> Vec<Asleep> {
    let Ok(entries) = fs::read_dir(dir) else {
        return vec![];
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let session = entry.file_name().to_str()?.to_string();
            let note = fs::symlink_metadata(entry.path())
                .ok()
                .filter(|metadata| metadata.is_file())
                .and_then(|_| util::read_json(&entry.path()))
                .and_then(|note| Asleep::from_note(session.clone(), &note));
            if note.is_none() {
                // Missing notes revoke pending handoffs even while host IPC is unavailable.
                let _ = fs::remove_file(entry.path());
                util::log_event("WARN", &format!("unreadable sleep note {session}"));
            }
            note
        })
        .collect()
}

pub fn asleep() -> Vec<Asleep> {
    notes_in(&paths::asleep_dir())
}

pub fn forget(session: &str) {
    forget_in(&paths::asleep_dir(), session);
}

fn forget_in(dir: &Path, session: &str) {
    if session.is_empty()
        || !session
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return;
    }
    let path = dir.join(session);
    // A process report is not an acknowledgement of committed team binding. The wake waiter
    // owns that cleanup; expiry and explicit exit can still revoke by deleting the note.
    if util::read_json(&path).is_some_and(|note| note.get("team_resume").is_some()) {
        return;
    }
    let _ = fs::remove_file(path);
}

fn expire_in(dir: &Path, now: i64) -> Vec<Asleep> {
    let expired: Vec<_> = notes_in(dir)
        .into_iter()
        .filter(|note| now - note.slept_at >= EXPIRY_S)
        .collect();
    for note in &expired {
        let _ = fs::remove_file(dir.join(&note.session));
    }
    expired
}

pub fn expire(now: i64) -> Vec<Asleep> {
    expire_in(&paths::asleep_dir(), now)
}

fn minutes_from(setting: Result<String, String>) -> i64 {
    let text = match setting {
        Ok(text) => text,
        Err(error) => {
            util::log_event(
                "WARN",
                &format!("idle-sleep-after: {error}; using {DEFAULT_AFTER_MIN} minutes"),
            );
            return DEFAULT_AFTER_MIN;
        }
    };
    if text.trim().is_empty() {
        return DEFAULT_AFTER_MIN;
    }
    match text.trim().parse::<i64>() {
        Ok(minutes) if minutes >= MIN_AFTER_MIN => minutes,
        _ => {
            util::log_event(
                "WARN",
                &format!(
                    "idle-sleep-after {text:?} is not a number of minutes from {MIN_AFTER_MIN}; using {DEFAULT_AFTER_MIN}"
                ),
            );
            DEFAULT_AFTER_MIN
        }
    }
}

/// Minutes a session may sit at its prompt before it sleeps (the idle-sleep-after setting).
pub fn after_minutes() -> i64 {
    minutes_from(settings::launch_value("idle-sleep-after"))
}

pub struct SleepContext {
    pub now: i64,
    pub enabled: bool,
    pub after_s: i64,
    pub viewed: String,
}

#[derive(Debug, PartialEq)]
pub enum SleepBlock {
    Disabled,
    Asleep,
    Restarting,
    External,
    Terminal,
    Unsupported,
    Viewed,
    NotReady,
    Busy,
    PendingMessage,
    Idle { remaining: i64 },
}

impl fmt::Display for SleepBlock {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("idle sleep disabled"),
            Self::Asleep => formatter.write_str("already asleep"),
            Self::Restarting => formatter.write_str("restarting"),
            Self::External => formatter.write_str("no terminal target"),
            Self::Terminal => formatter.write_str("terminal-managed session"),
            Self::Unsupported => formatter.write_str("its agent cannot sleep"),
            Self::Viewed => formatter.write_str("viewed in the cockpit"),
            Self::NotReady => formatter.write_str("not waiting at its prompt"),
            Self::Busy => formatter.write_str("assigned todo still in progress"),
            Self::PendingMessage => formatter.write_str("manual message queued"),
            Self::Idle { remaining } => write!(formatter, "idle window: {remaining}s remaining"),
        }
    }
}

pub fn block(
    session: &Session,
    context: &SleepContext,
    busy: bool,
    has_pending_message: bool,
) -> Option<SleepBlock> {
    if !context.enabled {
        return Some(SleepBlock::Disabled);
    }
    if session.asleep.is_some() {
        return Some(SleepBlock::Asleep);
    }
    if session.restarting {
        return Some(SleepBlock::Restarting);
    }
    if session.is_external() {
        return Some(SleepBlock::External);
    }
    if session.terminal {
        return Some(SleepBlock::Terminal);
    }
    if !session.agent().can(Feature::Sleep) {
        return Some(SleepBlock::Unsupported);
    }
    if session.session == context.viewed {
        return Some(SleepBlock::Viewed);
    }
    // activity_label ignores needs_input, which also keeps a session awake.
    if sessions::activity_label(session) != "Ready" || session.needs_input {
        return Some(SleepBlock::NotReady);
    }
    if busy {
        return Some(SleepBlock::Busy);
    }
    if has_pending_message {
        return Some(SleepBlock::PendingMessage);
    }
    let idle = context.now.saturating_sub(session.since);
    if idle < context.after_s {
        return Some(SleepBlock::Idle {
            remaining: context.after_s - idle,
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::{self, Session};
    use std::fs;
    use std::path::PathBuf;

    const NOW: i64 = 1_800_000_000;
    const HOUR_S: i64 = 3600;

    fn context() -> SleepContext {
        SleepContext {
            now: NOW,
            enabled: true,
            after_s: HOUR_S,
            viewed: String::new(),
        }
    }

    fn idle_session() -> Session {
        Session {
            session: "aaaaaaaa-0000-4000-8000-000000000000".into(),
            state: "waiting".into(),
            agent: "claude".into(),
            tmux_pane: "%1".into(),
            since: NOW - HOUR_S,
            ..Session::default()
        }
    }

    fn scratch_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("sleep-test-{name}-{}", crate::util::token_hex(6)));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_note(dir: &std::path::Path, session: &str, slept_at: i64) {
        let note = serde_json::json!({
            "agent": "opencode", "project": "vitals", "cwd": "/home/theo/vitals", "title": "Fix the chart",
            "idle_since": slept_at - HOUR_S, "slept_at": slept_at, "remote": true, "started": slept_at - 5 * HOUR_S,
        });
        fs::write(dir.join(session), note.to_string()).unwrap();
    }

    #[test]
    fn block_allows_a_ready_session_idle_past_the_delay() {
        assert_eq!(block(&idle_session(), &context(), false, false), None);
    }

    #[test]
    fn block_lets_a_pinned_session_sleep() {
        let pinned = Session {
            pinned: true,
            ..idle_session()
        };
        assert_eq!(block(&pinned, &context(), false, false), None);
    }

    #[test]
    fn block_reasons() {
        type Change = fn(&mut Session, &mut SleepContext, &mut bool, &mut bool);
        let cases: [(&str, Change, SleepBlock); 13] = [
            (
                "disabled",
                |_, context, _, _| context.enabled = false,
                SleepBlock::Disabled,
            ),
            (
                "asleep",
                |session, _, _, _| session.asleep = Some(NOW),
                SleepBlock::Asleep,
            ),
            (
                "restarting",
                |session, _, _, _| session.restarting = true,
                SleepBlock::Restarting,
            ),
            (
                "external",
                |session, _, _, _| session.tmux_pane.clear(),
                SleepBlock::External,
            ),
            (
                "terminal",
                |session, _, _, _| session.terminal = true,
                SleepBlock::Terminal,
            ),
            (
                "codex",
                |session, _, _, _| session.agent = "codex".into(),
                SleepBlock::Unsupported,
            ),
            (
                "viewed",
                |session, context, _, _| context.viewed = session.session.clone(),
                SleepBlock::Viewed,
            ),
            (
                "working",
                |session, _, _, _| session.state = "working".into(),
                SleepBlock::NotReady,
            ),
            (
                "needs input",
                |session, _, _, _| session.needs_input = true,
                SleepBlock::NotReady,
            ),
            (
                "awaiting approval",
                |session, _, _, _| session.ask_to_run = "approval".into(),
                SleepBlock::NotReady,
            ),
            ("busy", |_, _, busy, _| *busy = true, SleepBlock::Busy),
            (
                "pending message",
                |_, _, _, has_pending_message| *has_pending_message = true,
                SleepBlock::PendingMessage,
            ),
            (
                "idle one second short",
                |session, _, _, _| session.since = NOW - HOUR_S + 1,
                SleepBlock::Idle { remaining: 1 },
            ),
        ];
        for (name, change, expected) in cases {
            let (mut session, mut context, mut busy, mut has_pending_message) =
                (idle_session(), context(), false, false);
            change(
                &mut session,
                &mut context,
                &mut busy,
                &mut has_pending_message,
            );
            assert_eq!(
                block(&session, &context, busy, has_pending_message),
                Some(expected),
                "{name}"
            );
        }
    }

    #[test]
    fn notes_round_trip_into_cards() {
        let dir = scratch_dir("notes");
        write_note(&dir, "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L", NOW);
        let notes = notes_in(&dir);
        assert_eq!(notes.len(), 1);
        let card = notes[0].card();
        assert_eq!(card.session, "ses_3f2a9c0b1ffeQkTz8mWn4rJd2L");
        assert_eq!(card.asleep, Some(NOW));
        assert_eq!(card.since, NOW - HOUR_S);
        assert_eq!(card.started, Some(NOW - 5 * HOUR_S));
        assert_eq!(card.task, "Fix the chart");
        assert_eq!(card.agent, "opencode");
        assert!(card.facts.remote);
        assert!(!card.is_external());
        assert!(!sessions::is_idle(&card, NOW + 10 * 86_400));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn team_note_is_not_forgotten_before_binding_acknowledgement() {
        let dir = scratch_dir("team-binding");
        write_note(&dir, "teammate", NOW);
        let path = dir.join("teammate");
        let mut note = util::read_json(&path).unwrap();
        note["session"] = serde_json::json!("teammate");
        note["team_resume"] = serde_json::json!("private-permit");
        fs::write(&path, note.to_string()).unwrap();
        let card = notes_in(&dir).pop().unwrap().card();
        assert!(!format!("{card:?}").contains("private-permit"));
        forget_in(&dir, "teammate");
        assert!(
            path.exists(),
            "a new process report alone does not acknowledge team binding"
        );
        assert!(
            expire_in(&dir, NOW + EXPIRY_S)
                .iter()
                .any(|note| note.session == "teammate")
        );
        assert!(
            !path.exists(),
            "expiry still revokes permission by deleting the note"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_team_note_is_removed_without_exposing_the_permit() {
        let dir = scratch_dir("invalid-team-note");
        write_note(&dir, "teammate", NOW);
        let path = dir.join("teammate");
        let mut note = util::read_json(&path).unwrap();
        note["team_resume"] = serde_json::json!("private-permit");
        note["session"] = serde_json::json!("different-session");
        fs::write(&path, note.to_string()).unwrap();
        assert!(notes_in(&dir).is_empty());
        assert!(!path.exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn notes_skip_unreadable_files() {
        let dir = scratch_dir("unreadable");
        write_note(&dir, "readable", NOW);
        fs::write(dir.join("garbled"), "{not json").unwrap();
        fs::write(dir.join("incomplete"), r#"{"agent": "claude"}"#).unwrap();
        let sessions: Vec<_> = notes_in(&dir)
            .into_iter()
            .map(|note| note.session)
            .collect();
        assert_eq!(sessions, ["readable"]);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn expire_drops_notes_a_week_old() {
        let dir = scratch_dir("expire");
        write_note(&dir, "a-week-old", NOW - EXPIRY_S);
        write_note(&dir, "almost-a-week-old", NOW - EXPIRY_S + 3600);
        let expired = expire_in(&dir, NOW);
        assert_eq!(
            expired
                .iter()
                .map(|note| note.session.as_str())
                .collect::<Vec<_>>(),
            ["a-week-old"]
        );
        assert!(!dir.join("a-week-old").exists());
        assert!(dir.join("almost-a-week-old").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn after_minutes_defaults_and_floors() {
        for (setting, expected) in [
            (Ok(String::new()), 60),
            (Ok("45".into()), 45),
            (Ok("30".into()), 30),
            (Ok("10".into()), 60),
            (Ok("abc".into()), 60),
            (Err("invalid settings".into()), 60),
        ] {
            assert_eq!(minutes_from(setting.clone()), expected, "{setting:?}");
        }
    }
}
