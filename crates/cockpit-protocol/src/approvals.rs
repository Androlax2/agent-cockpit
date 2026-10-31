//! Wire views of what agents ask the owner to do (#34, report 17): `ask run` requests bound
//! to a revision and digest, and `ask guide` guides walked one step at a time. Paths travel
//! as UTF-8 text: the daemon refuses a working directory that is not.

use serde::{Deserialize, Serialize};

/// Text as the owner must see it: hiding characters escaped as `\u{…}`, runs of blank
/// lines collapsed, and every non-ASCII character named (`"а U+0430"`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownText {
    pub text: String,
    pub has_hidden_characters: bool,
    pub non_ascii: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStateView {
    Asking,
    /// Nobody waited for the answer any more; it can no longer be decided.
    Expired,
    Declined,
    Approved,
    Running,
    Finished,
    /// Approved, but the run never started (a restart, or the directory changed).
    NotRun,
    /// The daemon stopped while the command ran; whether it completed is unknown.
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRequestView {
    pub id: i64,
    pub session: String,
    /// Grows by one with each owner edit; a decision names the revision it was made on.
    pub revision: i64,
    pub state: RunStateView,
    /// The exact command of this revision.
    pub command: String,
    /// What the agent asked, when the owner edited it since.
    pub asked_command: Option<String>,
    /// Canonical, absolute.
    pub cwd: String,
    pub reason: String,
    /// The command, directory and reason under the display rules.
    pub shown_command: ShownText,
    pub shown_cwd: ShownText,
    pub shown_reason: ShownText,
    /// The asking agent's project, when the daemon knows it.
    pub project: Option<String>,
    /// The directory is not inside the project (always so when no project is known).
    pub outside_project: bool,
    /// SHA-256 of this revision's directory and command; an approval names it.
    pub digest: String,
    /// Seconds since 1970.
    pub asked_at: i64,
    pub expires_at: i64,
    /// A running command whose output stopped mid-line: it probably waits for input.
    pub waiting_for_input: bool,
    pub result: Option<RunResultView>,
    /// Why the request ended without a result, or what is known about it.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunResultView {
    pub exit_code: i32,
    /// The last lines, terminal controls stripped and secrets redacted.
    pub output: String,
    pub omitted_lines: u64,
    /// Bytes dropped from the front to keep the output bounded.
    pub truncated_bytes: u64,
    /// Secrets replaced in `output`.
    pub redacted: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GuideStateView {
    Asking,
    Expired,
    Done,
    Stuck,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuideView {
    pub id: i64,
    pub session: String,
    pub title: ShownText,
    pub cwd: String,
    pub project: Option<String>,
    pub state: GuideStateView,
    /// Index of the step shown now; an answer names it.
    pub current_step: usize,
    pub steps: Vec<GuideStepView>,
    pub stuck_note: Option<String>,
    pub asked_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuideStepView {
    pub section: Option<ShownText>,
    pub text: ShownText,
    pub open: Vec<ShownText>,
    pub copy: Vec<ShownText>,
    pub ask: Option<ShownText>,
    pub run: Option<ShownText>,
    pub answer: Option<String>,
    /// The run request a `run:` step filed when it was answered.
    pub run_request: Option<i64>,
}
