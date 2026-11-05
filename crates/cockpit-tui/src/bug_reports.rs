//! The `B` view (#471, #847): the bug report drafts the feed carries, the ones a send opened
//! apart from the ones still waiting for one, and the issue of a draft recorded in place with
//! `bug-report-sent`. Review and the send itself stay in the CLI (`torlek bug-report`).

use cockpit_protocol::{BugReportView, Command};

use crate::{Board, Key};

#[derive(Debug, Default)]
pub(crate) struct BugReports {
    /// The draft Enter records the issue of; `None`, or one the feed took off, is the first
    /// listed.
    selected: Option<i64>,
    recording: Option<Recording>,
    /// What the last issue recorded here did. Its draft left the list, so the view says it
    /// until it closes.
    recorded: Option<String>,
}

/// The issue the owner types for one draft.
#[derive(Debug)]
struct Recording {
    report: i64,
    issue: String,
    /// Why the last Enter recorded nothing, for the owner.
    problem: Option<String>,
    /// The `bug-report-sent` on its way: Enter sends no second one, and only its answer
    /// closes or annotates the input.
    submitted: Option<Command>,
}

pub(crate) enum BugReportsOutcome {
    Closed,
    Handled {
        used: &'static str,
        send: Vec<Command>,
    },
}

fn handled(used: &'static str) -> BugReportsOutcome {
    BugReportsOutcome::Handled {
        used,
        send: Vec::new(),
    }
}

/// Whether a send handed the owner the draft's issue: it may be submitted, nothing says so.
pub(crate) fn is_opened(draft: &BugReportView) -> bool {
    draft.opened_at.is_some()
}

/// The drafts in the order the view lists them: the ones a send opened, then the ones still
/// waiting for one, each oldest first.
fn listed(board: &Board) -> Vec<&BugReportView> {
    let mut drafts = board.bug_report_drafts();
    drafts.sort_by_key(|draft| !is_opened(draft));
    drafts
}

impl BugReports {
    pub(crate) fn key(&mut self, key: Key, board: &Board) -> BugReportsOutcome {
        if let Some(recording) = &mut self.recording {
            return match key {
                Key::Esc => {
                    self.recording = None;
                    handled("bug-report-record-cancel")
                }
                _ if recording.submitted.is_some() => handled("bug-report-record-pending"),
                Key::Enter => recording.submit(),
                Key::Backspace => {
                    recording.issue.pop();
                    handled("bug-report-record-edit")
                }
                Key::Char(character) => {
                    recording.issue.push(character);
                    handled("bug-report-record-edit")
                }
                _ => handled("bug-report-record-edit"),
            };
        }
        let drafts = listed(board);
        let at = self.selected_position(&drafts);
        match (key, at) {
            (Key::Esc | Key::Char('q'), _) => BugReportsOutcome::Closed,
            (Key::Char('j') | Key::Down, Some(at)) => {
                self.selected = Some(drafts[(at + 1).min(drafts.len() - 1)].id);
                handled("bug-report-move")
            }
            (Key::Char('k') | Key::Up, Some(at)) => {
                self.selected = Some(drafts[at.saturating_sub(1)].id);
                handled("bug-report-move")
            }
            (Key::Enter, Some(at)) => {
                self.recording = Some(Recording {
                    report: drafts[at].id,
                    issue: String::new(),
                    problem: None,
                    submitted: None,
                });
                handled("bug-report-record")
            }
            _ => handled("unbound"),
        }
    }

    /// Where the selected draft is in `drafts`; `None` when none is listed.
    fn selected_position(&self, drafts: &[&BugReportView]) -> Option<usize> {
        if drafts.is_empty() {
            return None;
        }
        let selected = self
            .selected
            .and_then(|selected| drafts.iter().position(|draft| draft.id == selected));
        Some(selected.unwrap_or(0))
    }

    /// The daemon recorded `report` as sent for `command`.
    pub(crate) fn recorded(&mut self, command: &Command, report: &BugReportView) {
        let is_answered = self
            .recording
            .as_ref()
            .is_some_and(|recording| recording.submitted.as_ref() == Some(command));
        if is_answered {
            self.recording = None;
            self.recorded = Some(recorded_text(report));
        }
    }

    /// The daemon refused `command`.
    pub(crate) fn failed(&mut self, command: &Command, problem: &str) {
        if let Some(recording) = &mut self.recording
            && recording.submitted.as_ref() == Some(command)
        {
            recording.submitted = None;
            recording.problem = Some(problem.to_owned());
        }
    }

    /// The view's text: the drafts one line each under what they wait for, how the owner
    /// reviews and sends them in the CLI (the designed view is Beta 3's, #465), and the issue
    /// being recorded.
    pub(crate) fn text(&self, board: &Board) -> String {
        let drafts = listed(board);
        let selected = self.selected_position(&drafts);
        let mut lines = Vec::new();
        for (position, draft) in drafts.iter().enumerate() {
            let is_first_of_its_group =
                position == 0 || is_opened(drafts[position - 1]) != is_opened(draft);
            if is_first_of_its_group {
                lines.push(
                    if is_opened(draft) {
                        "Opened to send, issue not recorded yet"
                    } else {
                        "Waiting for you"
                    }
                    .to_owned(),
                );
            }
            let marker = if selected == Some(position) { ">" } else { " " };
            lines.push(format!("{marker} {}", draft_line(draft)));
        }
        if drafts.is_empty() {
            lines.push("No bug report draft waits for you.".to_owned());
            lines.push(String::new());
            lines.push("Esc, then b reports a Torlek bug.".to_owned());
        } else {
            lines.push(String::new());
            lines.push("Review one: torlek bug-report show ID".to_owned());
            lines.push(
                "Send it: torlek bug-report send ID   Drop it: torlek bug-report discard ID"
                    .to_owned(),
            );
            lines.push(
                "Submitted one in your browser? j and k select it, Enter records its issue."
                    .to_owned(),
            );
        }
        if let Some(recording) = &self.recording {
            lines.push(String::new());
            lines.push(format!(
                "Issue of bug report {}, its URL or its number: {}",
                recording.report, recording.issue
            ));
            lines.push(
                if recording.submitted.is_some() {
                    "Recording…"
                } else {
                    "Enter records it; Esc goes back to the list."
                }
                .to_owned(),
            );
            if let Some(problem) = &recording.problem {
                lines.push(format!("Cannot record: {problem}"));
            }
        }
        if let Some(recorded) = &self.recorded {
            lines.push(String::new());
            lines.push(format!("Done: {recorded}"));
        }
        lines.join("\n")
    }
}

impl Recording {
    fn submit(&mut self) -> BugReportsOutcome {
        let issue = self.issue.trim();
        if issue.is_empty() {
            self.problem = Some("give the issue's URL or its number".to_owned());
            return handled("bug-report-record");
        }
        let command = Command::BugReportSent {
            report: self.report,
            issue_url: Some(issue.to_owned()),
        };
        self.submitted = Some(command.clone());
        self.problem = None;
        BugReportsOutcome::Handled {
            used: "bug-report-sent",
            send: vec![command],
        }
    }
}

/// What `bug-report-sent` did, for the view and the log.
pub(crate) fn recorded_text(report: &BugReportView) -> String {
    match &report.issue_url {
        Some(url) => format!("bug report {} is recorded as {url}", report.id),
        None => format!(
            "bug report {} is recorded as sent, without its issue",
            report.id
        ),
    }
}

fn draft_line(draft: &BugReportView) -> String {
    let mut line = format!("{}  {}  {}", draft.id, draft.origin.as_str(), draft.title);
    if draft.occurrences > 1 {
        line.push_str(&format!("  ({} times)", draft.occurrences));
    }
    if let Some(session) = &draft.session {
        line.push_str(&format!("  from {session}"));
    }
    line
}
