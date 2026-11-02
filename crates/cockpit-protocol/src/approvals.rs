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
    /// Why `text` differs from what the agent wrote, each kind once (#156). Every client
    /// (TUI, phone) shows these as warnings.
    #[serde(default)]
    pub warnings: Vec<HiddenView>,
}

/// A kind of character, or run of lines, the display rules escaped or collapsed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HiddenView {
    /// Direction-changing characters (`U+202E` shows `fdp` as `pdf`).
    BidiControl,
    /// Zero-width and other characters that take no room on screen.
    Invisible,
    /// Control codes other than the newline: escapes, carriage returns, tabs.
    Control,
    /// Two or more blank lines in a row, collapsed into one marked line.
    BlankLines,
    /// A kind this client does not know yet; show it as a warning all the same.
    #[serde(other)]
    Unknown,
}

/// The start of a long shown command, for panes and screens that fold it behind an explicit
/// "more"; the whole is `shown_command`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FoldedView {
    pub preview: String,
    /// Whole lines after the preview.
    pub more_lines: usize,
    /// Characters after the preview, its line breaks included.
    pub more_chars: usize,
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
    /// `shown_command` folded, when it is too long to show whole.
    #[serde(default)]
    pub folded_command: Option<FoldedView>,
    /// The command runs `timeout` without `--foreground`: a prompt in it would stop it.
    #[serde(default)]
    pub timeout_without_foreground: bool,
    /// The approval shell, when it has no parse-only mode the daemon knows (nushell's `-n`
    /// is not one): the command reached the owner without a syntax check (#494).
    #[serde(default)]
    pub unchecked_shell: Option<String>,
    /// Build files and scripts changed in the project's checkout outside its default branch that the
    /// command runs (Makefiles for `make`, `build.rs` for `cargo`, files it names). Read for
    /// the owner's views of an asking request; `None` when it was not or could not be read.
    #[serde(default)]
    pub changed_build_files: Option<Vec<String>>,
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
    pub shown_cwd: ShownText,
    pub project: Option<String>,
    /// The directory is outside the asking agent's project, or the project is unknown.
    pub outside_project: bool,
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
