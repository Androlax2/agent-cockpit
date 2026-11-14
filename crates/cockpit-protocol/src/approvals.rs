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
    /// The folders a confined run may write besides its checkout and the hosts it may reach
    /// (#1387), as the agent asked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    /// What the run may do once approved, one line each under the display rules, as the
    /// daemon's settings now confine runs (#1387); an unconfined run says so.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<ShownText>,
    /// SHA-256 of this revision's directory and command, and of the `writes` and `hosts`
    /// when it names any; an approval names it. In the owner's views of a request that runs
    /// files its session can edit (`pinned`, `root_runs`), bound to those files as the view
    /// read them: an approval names what it was shown.
    pub digest: String,
    /// Seconds since 1970.
    pub asked_at: i64,
    pub expires_at: i64,
    /// A running command idle at a prompt-like unfinished line: it probably waits for input.
    pub waiting_for_input: bool,
    /// While `waiting_for_input`: what the command asks (#1010), the output's last line as a
    /// terminal shows it, terminal controls stripped, secrets redacted, its end when long, and
    /// under the display rules. `None` otherwise, or when the daemon cannot show that line.
    #[serde(default)]
    pub waiting_prompt: Option<ShownText>,
    /// A sudo in the running command asks for the owner's password through Torlek's askpass
    /// helper (#1374): the oldest such ask, answered with `approval-sudo-password`. The
    /// command then also reads `waiting_for_input`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sudo_ask: Option<SudoAskView>,
    /// While the command runs: sudo processes in it that ended having asked the owner nothing
    /// (#1374). Its result counts them once it finished.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub sudo_ran_unasked: u64,
    /// While the command runs: how many times a secret the owner typed for it appeared in its
    /// output and was redacted (#1386). Its result counts them once it finished.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub typed_secret_redacted: u64,
    /// What this revision's sudo runs that the asking session can change (#1383), recorded
    /// before the owner read it. In the owner's views only; `None` there when the revision
    /// was not checked (a start request, or one filed before the check).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_runs: Option<RootRunsView>,
    /// What the rule scan found in this revision (#1388), high first, recorded before the
    /// owner read it: empty reads "no known risky pattern", never "safe". In the owner's views
    /// only; `None` there when the revision was not scanned (a start request, or one filed
    /// before the scan).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub findings: Option<Vec<FindingView>>,
    /// The files any part of this revision runs as code that the asking session can change
    /// (#1387), each hashed before the owner read it: one that changes before the run starts
    /// refuses it. Those sudo runs are in `root_runs` too. In the owner's views only; `None`
    /// there when the revision was not checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pinned: Option<Vec<RootFileView>>,
    pub result: Option<RunResultView>,
    /// Why the request ended without a result, or what is known about it.
    pub note: Option<String>,
    /// The owner's decision, once made (#657); `None` while asking or once expired.
    #[serde(default)]
    pub decision: Option<RunDecisionView>,
    /// Only on the answer to an `AskRun`: the session already had an identical ask pending
    /// (#1335), so this is that request, nothing new was filed and the new reason was dropped.
    #[serde(default)]
    pub already_asked: bool,
}

/// What a sudo in a running command asks the owner (#1374).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SudoAskView {
    /// Counts from 1 in each run; the answer names it.
    pub ask: u64,
    /// sudo's prompt as a terminal would show it, redacted, under the display rules.
    pub prompt: ShownText,
}

/// What a run request's sudo runs that the asking session can change (#1383). It warns; the
/// owner decides. A run whose recorded file no longer has its hash is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootRunsView {
    /// Files sudo runs that the session can change, in the order the command names them.
    pub editable: Vec<RootFileView>,
    /// Part of what runs as root cannot be resolved from the command line (an expansion,
    /// `eval`, code read from a pipe): never shown as clean.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cannot_tell: bool,
}

/// One file a request runs, or its sudo runs, that the asking session can change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootFileView {
    /// Absolute.
    pub path: String,
    /// As the owner reads it: from the request's directory when inside it (`./x.sh`), under
    /// the display rules.
    pub shown_path: ShownText,
    /// SHA-256 when recorded; `None` when the file did not exist, which the session could
    /// still create.
    pub sha256: Option<String>,
    /// What the file holds now, read for the owner's views of an asking request; `None`
    /// otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub now: Option<FileNowView>,
}

impl RootFileView {
    /// The file no longer has the hash recorded: approving it refuses the run.
    pub fn changed_since_asked(&self) -> bool {
        self.now
            .as_ref()
            .is_some_and(|now| now.unreadable || now.sha256 != self.sha256)
    }
}

/// A file as it is now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileNowView {
    /// `None` when the file is not there.
    pub sha256: Option<String>,
    /// Its first 200 lines under the display rules; `None` when absent or not UTF-8 text.
    pub head: Option<ShownText>,
    /// Lines after `head`.
    pub more_lines: u64,
    /// It could not be read (not a regular file, no permission, too slow): neither `sha256`
    /// nor `head` say anything, and the run will be refused.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unreadable: bool,
}

/// One known-risky pattern the rule scan found in a run request (#1388). It warns; the owner
/// decides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindingView {
    /// The rule's id (`download-runs`).
    pub rule: String,
    pub severity: FindingSeverityView,
    /// The line the owner reads (`runs code it downloads`).
    pub reason: String,
    /// The text that matched, on one line, under the display rules; empty when nothing in
    /// particular matched.
    pub excerpt: ShownText,
    /// The script it is in, as the owner reads it (`./setup.sh`); `None` on the command line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<ShownText>,
    /// The script's line it starts on, from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u64>,
}

/// How much a finding weighs: high ones come first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindingSeverityView {
    High,
    Warn,
    /// A severity this client does not know yet; shown as a finding all the same.
    #[serde(other)]
    Unknown,
}

/// A finding as the card and `approval show` read it, its matching text `excerpt` (the
/// finding's, or a shortened one) marked with backticks: `High: runs code it downloads:
/// `curl … | sh``, `High: reads secrets in ./setup.sh:4: `cat ~/.ssh/id_rsa``.
pub fn finding_line(finding: &FindingView, excerpt: &str) -> String {
    let severity = match finding.severity {
        FindingSeverityView::High => "High",
        FindingSeverityView::Warn => "Warn",
        FindingSeverityView::Unknown => "Finding",
    };
    let place = match (&finding.file, finding.line) {
        (Some(file), Some(line)) => format!(" in {}:{line}", file.text),
        (Some(file), None) => format!(" in {}", file.text),
        (None, _) => String::new(),
    };
    if excerpt.is_empty() {
        format!("{severity}: {}{place}", finding.reason)
    } else {
        format!("{severity}: {}{place}: `{excerpt}`", finding.reason)
    }
}

/// What a scanned request without a finding reads: a clean scan proves nothing safe.
pub const NO_KNOWN_RISKY_PATTERN: &str = "Scan: no known risky pattern";

impl RunRequestView {
    /// The pinned files not already shown as run by sudo.
    pub fn pinned_beyond_root(&self) -> impl Iterator<Item = &RootFileView> {
        let root = self.root_runs.as_ref().map(|runs| runs.editable.as_slice());
        self.pinned.iter().flatten().filter(move |file| {
            !root.is_some_and(|root| root.iter().any(|sudo| sudo.path == file.path))
        })
    }
}

/// The card's line for a file a request runs that changed after it was recorded.
pub fn changed_since_asked_warning(path: &str) -> String {
    format!("{path} changed since it was asked: approving it refuses the run")
}

/// One decided run request as `approval list --all` reads it (#1252): what it was, how long
/// the owner took and how it ended, without the output `approval show` carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecidedRunView {
    pub id: i64,
    pub session: String,
    pub state: RunStateView,
    /// SHA-256 of the decided revision's directory and command, as an approval names it.
    pub digest: String,
    /// The decided revision's command under the display rules.
    pub shown_command: ShownText,
    /// Seconds since 1970.
    pub asked_at: i64,
    pub decision: RunDecisionView,
    /// `None` when the command never ran or its outcome is unknown.
    pub exit_code: Option<i32>,
}

/// The owner's decision on a run request (#657): only the owner decides, on the host-only
/// socket; `through` is the client the owner used, as that client named itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunDecisionView {
    pub approved: bool,
    /// Seconds since 1970.
    pub at: i64,
    /// `None` for a client that did not name itself, or a decision recorded before #657.
    pub through: Option<DecidedThroughView>,
    /// The owner's "always" approval (#1387) that approved the ask as it was filed, with no
    /// prompt; `None` when the owner decided it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub always: Option<i64>,
}

/// One "always" approval the owner made (#1387): its project's later asks of exactly this
/// command line, in this directory, with this card, from this checkout, run with no prompt
/// while each of `files` has the hash shown. A changed file or argument asks again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlwaysApprovalView {
    pub id: i64,
    pub project: String,
    /// The checkout its asks come from, canonical.
    pub project_root: String,
    pub command: String,
    pub cwd: String,
    /// The command and directory under the display rules.
    pub shown_command: ShownText,
    pub shown_cwd: ShownText,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hosts: Vec<String>,
    /// The files it is bound to, each with the hash it had when the owner approved it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<RootFileView>,
    /// The request whose approval made it.
    pub request: i64,
    /// Seconds since 1970.
    pub granted_at: i64,
    pub granted_through: Option<DecidedThroughView>,
    /// `None` while it still approves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<i64>,
}

/// The owner client a decision came through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecidedThroughView {
    /// A key in the terminal UI's approval pane.
    Tui,
    /// `torlek approval approve` or `decline`.
    Cli,
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
    /// Retention cleared the output long after the run (#406), or the owner purged it
    /// (#1160); `output` is empty.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub output_retired: bool,
    /// The owner stopped the command (#1301); `exit_code` is what the stop left, 143 after
    /// `SIGTERM`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopped_by_owner: bool,
    /// sudo processes in the command that ended having asked the owner nothing, so may have
    /// reused an unlock (#1374): "sudo ran without asking you".
    #[serde(default, skip_serializing_if = "is_zero")]
    pub sudo_ran_unasked: u64,
    /// How many times a secret the owner typed for the command appeared in its output and
    /// was replaced by `[redacted]` (#1386): a sign the command handles it badly.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub typed_secret_redacted: u64,
    /// What the run's confinement refused it (#1387), each once: the path it could not read
    /// or write, the host it could not reach. Redacted, terminal controls stripped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limit_hits: Vec<String>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's `skip_serializing_if` passes a reference"
)]
fn is_zero(count: &u64) -> bool {
    *count == 0
}

impl RunRequestView {
    /// The sudo processes that ran without asking the owner so far: the result's total once
    /// the command finished, the live count while it runs (#1374).
    pub fn sudo_ran_unasked_so_far(&self) -> u64 {
        self.result
            .as_ref()
            .map_or(self.sudo_ran_unasked, |result| result.sudo_ran_unasked)
    }

    /// How many times a typed secret was redacted from the output so far: the result's total
    /// once the command finished, the live count while it runs (#1386).
    pub fn typed_secret_redacted_so_far(&self) -> u64 {
        self.result
            .as_ref()
            .map_or(self.typed_secret_redacted, |result| {
                result.typed_secret_redacted
            })
    }
}

/// What the owner and the agent read when a secret the owner typed for a run appeared
/// `count` times in its output (#1386): the same words in the pane, `approval show` and the
/// answer.
pub fn typed_secret_redacted_notice(count: u64) -> String {
    let times = if count == 1 { "time" } else { "times" };
    format!(
        "your typed secret appeared in the output {count} {times} and was redacted: the \
         command handles it badly"
    )
}

/// What the owner and the agent read when `count` sudo processes in a run ended having asked
/// the owner nothing (#1374): the same words in the pane, `approval show` and the answer.
pub fn sudo_ran_unasked_warning(count: u64) -> String {
    format!(
        "sudo ran without asking you: {count} sudo in the command asked nothing through \
         Torlek's askpass, so it may have reused an earlier unlock or been handed a password \
         by the command"
    )
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
