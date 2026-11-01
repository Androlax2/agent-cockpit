use crate::Error;
use crate::error::invalid_row;

/// A report's number, as `cockpit bug-report show N` names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReportId(pub i64);

/// Who opened a draft.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The owner, on the host: `cockpit bug-report new` or the TUI.
    Owner,
    /// A session, through `cockpit agent report-bug`. `requested_by_owner` is the session's
    /// claim that its owner asked for the report: it lifts the per-fingerprint limit, never
    /// the per-session one, and the owner still reviews the draft.
    Agent {
        session: String,
        requested_by_owner: bool,
    },
    /// The daemon, about a fault it saw itself: a panic, a stalled lane, a crash found at
    /// restart, an internal error repeated.
    Daemon,
}

impl Source {
    /// Whether one draft per fingerprint per day bounds this source. The limit stops agent
    /// and daemon loops, not the owner.
    pub(crate) fn is_limited(&self) -> bool {
        match self {
            Source::Owner => false,
            Source::Agent {
                requested_by_owner, ..
            } => !requested_by_owner,
            Source::Daemon => true,
        }
    }

    pub(crate) fn origin(&self) -> Origin {
        match self {
            Source::Owner => Origin::Owner,
            Source::Agent { .. } => Origin::Agent,
            Source::Daemon => Origin::Daemon,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Owner,
    Agent,
    Daemon,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Owner => "owner",
            Origin::Agent => "agent",
            Origin::Daemon => "daemon",
        }
    }

    pub(crate) fn parse(text: &str) -> Result<Origin, Error> {
        match text {
            "owner" => Ok(Origin::Owner),
            "agent" => Ok(Origin::Agent),
            "daemon" => Ok(Origin::Daemon),
            other => Err(invalid_row(format!("origin {other:?}"))),
        }
    }
}

/// What went wrong, as the fingerprint names it. Agents report the first four; the daemon
/// the internal, panic, stall and crash kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A `cockpit` command ran out of time.
    Timeout,
    /// The daemon did not answer at all.
    Unreachable,
    /// An `internal` error came back instead of a typed answer.
    Internal,
    Panic,
    /// A request ran past its lane's budget.
    Stall,
    /// The daemon ended without a clean stop, found at its next start.
    Crash,
    /// Anything else, or the owner's own report.
    Other,
}

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::Timeout,
        Kind::Unreachable,
        Kind::Internal,
        Kind::Panic,
        Kind::Stall,
        Kind::Crash,
        Kind::Other,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Timeout => "timeout",
            Kind::Unreachable => "unreachable",
            Kind::Internal => "internal",
            Kind::Panic => "panic",
            Kind::Stall => "stall",
            Kind::Crash => "crash",
            Kind::Other => "other",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|kind| kind.as_str() == text)
    }
}

/// Version, error kind and component: what makes two reports the same bug. It is shown in
/// the issue so an existing issue for it can be found instead of a duplicate filed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub version: String,
    pub kind: Kind,
    pub component: String,
}

/// The component of a report that names none.
pub const UNKNOWN_COMPONENT: &str = "unknown";

impl Fingerprint {
    /// `cockpit-<version>/<kind>/<component>`, the text the issue carries and a search finds.
    pub fn text(&self) -> String {
        format!(
            "cockpit-{}/{}/{}",
            self.version,
            self.kind.as_str(),
            self.component
        )
    }

    /// Whether the fingerprint tells one bug from another: `other` with no component would
    /// match every unspecific report, so a send looks for an existing issue only otherwise.
    pub fn is_specific(&self) -> bool {
        self.kind != Kind::Other || self.component != UNKNOWN_COMPONENT
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Waits for the owner's review; nothing left the machine.
    Draft,
    /// The owner sent it.
    Sent,
    Discarded,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Draft => "draft",
            Status::Sent => "sent",
            Status::Discarded => "discarded",
        }
    }

    pub(crate) fn parse(text: &str) -> Result<Status, Error> {
        match text {
            "draft" => Ok(Status::Draft),
            "sent" => Ok(Status::Sent),
            "discarded" => Ok(Status::Discarded),
            other => Err(invalid_row(format!("status {other:?}"))),
        }
    }
}

/// What a new report says. `title` defaults to the description's first line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReport {
    pub source: Source,
    pub kind: Kind,
    /// The part of Cockpit at fault, a plain name such as `todo-add` or `control/session-new`.
    pub component: Option<String>,
    pub title: Option<String>,
    pub description: String,
    /// How to reproduce it, when known; may be empty.
    pub steps: String,
}

/// What the daemon knows about itself, gathered from the owners of each fact: the
/// version that goes into the fingerprint, and the environment text `cockpit version --all`
/// prints (versions, platform, providers). It names no user, path or environment value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub version: String,
    pub environment: String,
}

/// The support bundle attached to a draft: written by the daemon after the draft exists, so
/// a report is never lost to a bundle that cannot be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bundle {
    /// Not written yet.
    Pending,
    /// The archive at `path`, holding exactly `contents` (`<bytes>\t<path>` per line).
    Written { path: String, contents: String },
    /// Why there is none; the report goes without it.
    Failed { problem: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub id: ReportId,
    pub origin: Origin,
    pub requested_by_owner: bool,
    /// The session that reported it, for an agent's report.
    pub session: Option<String>,
    pub status: Status,
    pub title: String,
    pub description: String,
    pub steps: String,
    pub fingerprint: Fingerprint,
    pub environment: String,
    pub bundle: Bundle,
    /// How many reports this draft holds: repeats the daily limit merged count here.
    pub occurrences: i64,
    /// The issue the owner filed with it, when the send returned one.
    pub issue_url: Option<String>,
    pub created_at: i64,
    pub changed_at: i64,
}

/// What filing a report did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filed {
    /// A new draft; the caller attaches its support bundle.
    Created(Report),
    /// A draft with the same fingerprint was opened today and still waits: this report
    /// counts as one more occurrence of it.
    Merged(Report),
    /// A report with the same fingerprint was sent or discarded today: nothing new opens
    /// until tomorrow.
    AlreadyHandled(Report),
}

impl Filed {
    pub fn report(&self) -> &Report {
        match self {
            Filed::Created(report) | Filed::Merged(report) | Filed::AlreadyHandled(report) => {
                report
            }
        }
    }
}

/// The owner's changes to a draft; `None` keeps a field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Edit {
    pub title: Option<String>,
    pub description: Option<String>,
    pub steps: Option<String>,
}
