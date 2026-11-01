use std::ffi::OsString;
use std::fmt;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use cockpit_processes::{Escalation, ExecutionId};
use cockpit_projects::CheckoutId;
use cockpit_provider_api::{ConversationId, ProviderName};

use crate::Error;

/// Random bytes in a session id: 48 bits, written as 12 hex digits after `s`.
const SESSION_ID_BYTES: usize = 6;
/// Longest design link; a URL, not a document.
const MAX_LINK_BYTES: usize = 2048;

/// A session's id: `s` and 12 hex digits. It names the session's tmux session, its checkout
/// occupancy and its identity on the agent channel, so it is one plain word.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(String);

impl SessionId {
    pub fn parse(text: &str) -> Result<SessionId, Error> {
        let is_well_formed = text.len() == 1 + SESSION_ID_BYTES * 2
            && text.starts_with('s')
            && text[1..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !is_well_formed {
            return Err(Error::InvalidSessionId {
                id: text.to_owned(),
            });
        }
        Ok(SessionId(text.to_owned()))
    }

    pub(crate) fn generate() -> Result<SessionId, Error> {
        let mut bytes = [0_u8; SESSION_ID_BYTES];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut random| random.read_exact(&mut bytes))
            .map_err(|source| Error::Io {
                action: "read",
                path: PathBuf::from("/dev/urandom"),
                source,
            })?;
        let digits: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(SessionId(format!("s{digits}")))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Where a session is in its life. Transitional states are stored too, so a daemon that
/// stopped in the middle of one finishes or repairs it at its next start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// A launch (start, restart, wake, resume or restore) is under way.
    Launching,
    /// Its agent runs in its terminal.
    Running,
    /// Its agent is being stopped; `to` is where the session goes once it is observed gone.
    Stopping { to: StopTarget },
    /// Its agent was stopped while idle; the conversation is woken on demand.
    Asleep,
    /// Over: the owner ended it, its agent exited, or it slept too long. `resume` reopens it.
    Ended,
    /// A launch failed; the reason says why. Never retried silently: `resume` or `end`.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopTarget {
    Asleep,
    Ended,
    /// A restart in place: the agent stops, then the same conversation launches again.
    Restart,
}

impl SessionState {
    pub(crate) fn column(self) -> &'static str {
        match self {
            SessionState::Launching => "launching",
            SessionState::Running => "running",
            SessionState::Stopping { .. } => "stopping",
            SessionState::Asleep => "asleep",
            SessionState::Ended => "ended",
            SessionState::Failed => "failed",
        }
    }

    pub(crate) fn stop_column(self) -> Option<&'static str> {
        match self {
            SessionState::Stopping { to } => Some(to.column()),
            _ => None,
        }
    }

    pub(crate) fn from_columns(state: &str, stopping_to: Option<&str>) -> Option<SessionState> {
        Some(match (state, stopping_to) {
            ("launching", None) => SessionState::Launching,
            ("running", None) => SessionState::Running,
            ("stopping", Some(to)) => SessionState::Stopping {
                to: StopTarget::from_column(to)?,
            },
            ("asleep", None) => SessionState::Asleep,
            ("ended", None) => SessionState::Ended,
            ("failed", None) => SessionState::Failed,
            _ => return None,
        })
    }
}

impl StopTarget {
    fn column(self) -> &'static str {
        match self {
            StopTarget::Asleep => "asleep",
            StopTarget::Ended => "ended",
            StopTarget::Restart => "restart",
        }
    }

    fn from_column(text: &str) -> Option<StopTarget> {
        [StopTarget::Asleep, StopTarget::Ended, StopTarget::Restart]
            .into_iter()
            .find(|target| target.column() == text)
    }
}

impl fmt::Display for SessionState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionState::Stopping { to } => write!(formatter, "stopping ({})", to.column()),
            state => formatter.write_str(state.column()),
        }
    }
}

/// Whether a launch is made to give the session work, which drops its finished label: a
/// start, a resume, the owner's restart, every wake. The others relaunch the same
/// conversation with no new input and keep the label: the restore on daemon start,
/// `restart --all` and its deferred restart (#269, decided in #312). A launch or restart a
/// crash interrupted is finished as `Work`: who asked for it is not kept, and dropping the
/// label is the safe direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchPurpose {
    Work,
    Upkeep,
}

/// Why a launch happened; each launch of a session is one row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchKind {
    Start,
    Restart,
    Wake,
    Resume,
    /// Reopened automatically when the daemon started after the agent was gone.
    Restore,
}

impl LaunchKind {
    pub(crate) fn column(self) -> &'static str {
        match self {
            LaunchKind::Start => "start",
            LaunchKind::Restart => "restart",
            LaunchKind::Wake => "wake",
            LaunchKind::Resume => "resume",
            LaunchKind::Restore => "restore",
        }
    }

    pub(crate) fn from_column(text: &str) -> Option<LaunchKind> {
        [
            LaunchKind::Start,
            LaunchKind::Restart,
            LaunchKind::Wake,
            LaunchKind::Resume,
            LaunchKind::Restore,
        ]
        .into_iter()
        .find(|kind| kind.column() == text)
    }
}

/// The latest launch of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub number: i64,
    pub kind: LaunchKind,
    /// The execution its agent runs as.
    pub execution: ExecutionId,
    /// Seconds since the Unix epoch.
    pub launched_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub provider: ProviderName,
    pub checkout: CheckoutId,
    /// Whether the session occupies its checkout (a clone); a main checkout is shared.
    pub holds_checkout: bool,
    pub working_dir: PathBuf,
    /// Known once the provider named it; without it the session cannot be resumed.
    pub conversation: Option<ConversationId>,
    pub design_link: Option<String>,
    pub state: SessionState,
    /// Why the session is in its state, for the owner: a failure, an exit, an expiry.
    pub reason: Option<String>,
    /// Counts every change; a client compares it to detect a stale view.
    pub revision: i64,
    /// Seconds since the Unix epoch.
    pub created_at: i64,
    pub changed_at: i64,
    pub launch: Option<Launch>,
    /// Where the latest launch made to give the session work lies among the database's
    /// commits ([`cockpit_turso::WriteTransaction::commit_order`]): a finish ordered after it
    /// counts for the finished label (#269, decided in #312; #273). `None` before any.
    pub working_launch_order: Option<i64>,
    /// `restart --all` deferred its restart until the agent waits at its prompt; the
    /// periodic pass restarts it then.
    pub restart_pending: bool,
}

/// `cockpit session new`: a directory of a registered project, in place or in a new
/// parallel clone on `clone_branch`.
#[derive(Debug, Clone)]
pub struct StartRequest {
    pub dir: PathBuf,
    /// `None` when exactly one provider is available.
    pub provider: Option<String>,
    pub first_message: Option<String>,
    pub clone_branch: Option<String>,
}

/// What [`crate::Sessions::restore`] did with one session at daemon start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restored {
    /// Its agent still ran; the session is kept as it was.
    StillRunning(SessionId),
    /// Its agent was gone and the conversation was reopened.
    Reopened(SessionId),
    /// A stop that was under way was finished.
    StopFinished(SessionId, SessionState),
    /// A stop that was under way could not be finished, for this reason; the session stays
    /// as the error left it (`stopping`, or `running` when its agent did not stop), and
    /// `end` finishes it.
    StopNotFinished(SessionId, String),
    /// Reopening failed; the session is `failed` with this reason and is not retried.
    Failed(SessionId, String),
}

/// What one [`crate::Sessions::tick`] changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    /// Sessions whose agent exited by itself.
    pub exited: Vec<SessionId>,
    pub slept: Vec<SessionId>,
    /// Asleep sessions nobody woke within the expiry, now ended.
    pub expired: Vec<SessionId>,
    /// Sessions whose deferred restart ran once their agent waited at its prompt.
    pub restarted: Vec<SessionId>,
    /// Sessions a step failed for, with the reason; the next tick looks again.
    pub errors: Vec<(SessionId, String)>,
}

/// What [`crate::Sessions::restart_all`] did: only sessions whose start files changed since
/// their launch are restarted, now when their agent waits at its prompt, later otherwise.
#[derive(Debug, Default)]
pub struct RestartAll {
    pub restarted: Vec<Session>,
    /// Busy sessions (working, asking, or not yet reporting), with the start files that
    /// changed: each restarts once its agent reports waiting at its prompt.
    pub deferred: Vec<(Session, Vec<String>)>,
    pub failed: Vec<(SessionId, Error)>,
}

/// What wakes an asleep session; recorded as the reason of its move to launching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeCause {
    /// `cockpit session wake`.
    Owner,
    /// A client opened the session's terminal.
    Open,
    /// A message was sent to the session.
    Message,
    /// A todo was handed to the session.
    Todo,
    /// The owner woke it from the phone.
    Phone,
}

impl WakeCause {
    pub(crate) fn reason(self) -> &'static str {
        match self {
            WakeCause::Owner => "woken by the owner",
            WakeCause::Open => "woken on open",
            WakeCause::Message => "woken by a message",
            WakeCause::Todo => "woken by a todo",
            WakeCause::Phone => "woken from the phone",
        }
    }
}

/// Why a session may not sleep now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SleepRefusal {
    NotRunning(SessionState),
    /// Sleeping would lose the conversation: the provider cannot resume, or never named it.
    CannotResume(String),
    /// The agent reported it is working.
    Working,
    /// The agent asked the owner a question that is still open.
    NeedsInput,
    /// The agent never reported that it waits at its prompt.
    NotReady,
    /// Seconds left before the idle window ends.
    Idle {
        remaining: i64,
    },
}

impl fmt::Display for SleepRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SleepRefusal::NotRunning(state) => write!(formatter, "the session is {state}"),
            SleepRefusal::CannotResume(reason) => {
                write!(formatter, "it could not be woken: {reason}")
            }
            SleepRefusal::Working => formatter.write_str("its agent is working"),
            SleepRefusal::NeedsInput => formatter.write_str("its agent waits for an answer"),
            SleepRefusal::NotReady => {
                formatter.write_str("its agent has not reported waiting at its prompt")
            }
            SleepRefusal::Idle { remaining } => {
                write!(formatter, "idle window: {remaining}s remaining")
            }
        }
    }
}

/// The lifecycle's settings; [`Settings::default`] holds the product's values.
#[derive(Debug, Clone)]
pub struct Settings {
    /// The owner's idle window (how long an agent waits at its prompt before it is put to
    /// sleep), asleep expiry (how long a session may sleep before it is ended) and desktop
    /// session choice (whether agents get the owner's desktop session variables, as last
    /// given to [`crate::Sessions::import_desktop_environment`], after the base ones, #305).
    /// These are the start values only: the lifecycle reads them through
    /// `Sessions::owner_settings`, which `Sessions::apply_owner_settings` replaces (#354).
    pub owner: cockpit_config::SessionSettings,
    /// How an agent is stopped: `SIGTERM`, then `SIGKILL` to its whole containment.
    pub stop: Escalation,
    /// The window size of a new session's terminal until a client attaches.
    pub columns: u16,
    pub rows: u16,
    /// Variables every agent gets before its provider's own, such as `HOME` and `PATH`;
    /// the daemon passes its own values.
    pub base_environment: Vec<(OsString, OsString)>,
    /// The files agents read only at launch (`cockpit-config`); each launch records their
    /// state, and `restart --all` restarts the sessions whose files changed since.
    pub start_files: Vec<cockpit_config::StartFile>,
    /// How long one step that waits outside the daemon may take: the provider building its
    /// command, a tmux call, a file or process read. A step that runs out of it fails, so a
    /// hung launch ends and the owner can stop its session (#272).
    pub step_timeout: Duration,
    /// How many blocking steps may be outstanding at once, timed-out ones included until
    /// they return: a hung tmux or provider keeps its thread, and the runtime's blocking pool
    /// is shared with every capability (#325, report 03). At least 1.
    pub blocking_steps: usize,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            owner: cockpit_config::SessionSettings {
                // The current cockpit's `idle-sleep-after` default and expiry (sleep.rs).
                idle_sleep_after: Duration::from_secs(60 * 60),
                asleep_expiry: Duration::from_secs(7 * 24 * 60 * 60),
                desktop_session: crate::DesktopSession::Shared,
            },
            stop: Escalation {
                grace: Duration::from_secs(5),
                kill_wait: Duration::from_secs(5),
            },
            columns: 200,
            rows: 50,
            base_environment: Vec::new(),
            start_files: Vec::new(),
            step_timeout: Duration::from_secs(30),
            // An eighth of Tokio's default blocking pool (512), so a hung tmux leaves the
            // rest of the daemon its threads.
            blocking_steps: 64,
        }
    }
}

/// A design link as the owner gives it: an `http` or `https` URL, nothing else.
pub(crate) fn validate_link(link: &str) -> Result<(), Error> {
    let is_url = (link.starts_with("https://") || link.starts_with("http://"))
        && link.len() <= MAX_LINK_BYTES
        && link.chars().all(|character| character.is_ascii_graphic());
    if !is_url {
        return Err(Error::InvalidLink {
            link: link.to_owned(),
        });
    }
    Ok(())
}
