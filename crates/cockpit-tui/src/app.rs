//! The TUI's state machine: keys and the daemon's answers in, commands out. It never waits:
//! the runtime sends the commands and hands back each answer, so a slow daemon leaves the
//! list, the pane and the form responsive.

use std::collections::VecDeque;
use std::time::Instant;

use cockpit_protocol::{ChangeBatch, Command, ManagerPaneView, Reply};

use crate::board::{Board, FeedError};
use crate::form::{Form, FormOutcome};
use crate::pane::{Pane, PaneOutcome};
use crate::workflows::{Workflows, WorkflowsOutcome};

/// The log view keeps this many entries.
const LOG_ENTRIES: usize = 200;

/// A key, as the TUI reads it (report 05: presses only, never releases or repeats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    Backspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Focus {
    #[default]
    List,
    Pane,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    Help,
    Log,
}

/// Where tmux's keys were before the pane took them for the column (#345): the TUI gives
/// them back to the viewer once nothing waits, never to a viewer that did not have them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum TakenKeys {
    #[default]
    NotTaken,
    /// `manager-focus` is on its way; its answer says where the keys were.
    Asking,
    FromViewer,
}

/// A change to a session the owner must confirm with `y`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Restart(String),
    Exit(String),
    /// The sessions of the finished group as the owner saw it: `y` ends only these.
    ExitFinished(Vec<String>),
}

/// What one key did: the command it counts as in diagnostics (decision 17: every command
/// is counted so M6 prunes on data) and what to send to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Step {
    pub used: Option<&'static str>,
    pub send: Vec<Command>,
}

impl Step {
    fn used(name: &'static str) -> Step {
        Step {
            used: Some(name),
            send: Vec::new(),
        }
    }

    fn sending(name: &'static str, command: Command) -> Step {
        Step {
            used: Some(name),
            send: vec![command],
        }
    }
}

#[derive(Debug, Default)]
pub struct App {
    pub(crate) board: Board,
    pub(crate) selected: Option<String>,
    pub(crate) focus: Focus,
    pub(crate) pane: Pane,
    pub(crate) form: Option<Form>,
    pub(crate) workflows: Option<Workflows>,
    pub(crate) overlay: Option<Overlay>,
    pub(crate) confirm: Option<Confirm>,
    pub(crate) log: VecDeque<String>,
    /// The last failure, on the status line until a success of the same kind of command
    /// (or a session, request or guide answer). The same problem again, as every refresh
    /// while the daemon is down, is not logged again.
    pub(crate) failure: Option<Failure>,
    /// Why the change feed stopped: the sessions and checkouts shown are not current until
    /// a batch applies again.
    pub(crate) feed_problem: Option<String>,
    /// The daemon's profile (#196), in the list's title so a `dev` TUI never passes for the
    /// installed one's.
    pub(crate) profile: Option<String>,
    taken_keys: TakenKeys,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Failure {
    /// The failed command's wire kind.
    pub(crate) kind: String,
    pub(crate) problem: String,
}

impl App {
    pub fn new() -> App {
        App::default()
    }

    /// An app whose list's title names `profile`.
    pub fn for_profile(profile: String) -> App {
        App {
            profile: Some(profile),
            ..App::default()
        }
    }

    /// What the runtime asks the daemon on every refresh. Sessions and checkouts are not
    /// among them: they come from the change feed (#199).
    pub fn refresh_commands() -> Vec<Command> {
        vec![
            Command::ProjectList,
            Command::AgentReports,
            Command::ApprovalQueue,
        ]
    }

    /// What keeps the views open on screen current, sent with every refresh.
    pub fn view_refresh_commands(&self) -> Vec<Command> {
        self.workflows
            .as_ref()
            .map(Workflows::refresh_commands)
            .unwrap_or_default()
    }

    pub fn workflows(&self) -> Option<&Workflows> {
        self.workflows.as_ref()
    }

    pub fn board(&self) -> &Board {
        &self.board
    }

    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn pane(&self) -> &Pane {
        &self.pane
    }

    pub fn form(&self) -> Option<&Form> {
        self.form.as_ref()
    }

    pub fn overlay(&self) -> Option<Overlay> {
        self.overlay
    }

    pub fn log(&self) -> impl Iterator<Item = &str> {
        self.log.iter().map(String::as_str)
    }

    /// Applies a change-feed batch whole, or refuses it whole.
    pub fn apply_changes(&mut self, batch: &ChangeBatch) -> Result<(), FeedError> {
        self.board.apply(batch)?;
        self.feed_problem = None;
        self.keep_selection();
        Ok(())
    }

    /// The change feed stopped: on the status line until a batch applies, logged once per
    /// problem.
    pub fn feed_lost(&mut self, problem: String) {
        // A daemon outage fails the polled commands too: the same problem is logged once.
        let is_logged = self.feed_problem.as_ref() == Some(&problem)
            || self
                .failure
                .as_ref()
                .is_some_and(|failure| failure.problem == problem);
        if !is_logged {
            self.note(format!("change feed: {problem}"));
        }
        self.feed_problem = Some(problem);
    }

    pub fn key(&mut self, key: Key, now: Instant) -> Step {
        if let Some(overlay) = self.overlay {
            return match key {
                Key::Esc | Key::Char('q') => {
                    self.overlay = None;
                    Step::used("close-overlay")
                }
                Key::Char('?') if overlay == Overlay::Log => self.open(Overlay::Help),
                Key::Char('L') if overlay == Overlay::Help => self.open(Overlay::Log),
                _ => Step::used("unbound"),
            };
        }
        if let Some(form) = &mut self.form {
            return match form.key(key) {
                FormOutcome::Editing => Step::used("form-edit"),
                FormOutcome::Cancelled => {
                    self.form = None;
                    Step::used("new-session-cancel")
                }
                FormOutcome::Submit(command) => Step::sending("new-session", command),
            };
        }
        // The approval pane keeps its keys while the view is open, and Tab moves to it.
        if let Some(workflows) = &mut self.workflows
            && self.focus == Focus::List
            && (key != Key::Tab || workflows.is_typing())
        {
            return match workflows.key(key) {
                WorkflowsOutcome::Closed => {
                    self.workflows = None;
                    Step::used("workflows-close")
                }
                WorkflowsOutcome::Handled { used, send } => Step {
                    used: Some(used),
                    send,
                },
            };
        }
        if let Some(confirm) = self.confirm.take() {
            return match (key, confirm) {
                (Key::Char('y'), Confirm::Restart(session)) => {
                    Step::sending("restart", Command::SessionRestart { session })
                }
                (Key::Char('y'), Confirm::Exit(session)) => {
                    Step::sending("exit", Command::SessionEnd { session })
                }
                (Key::Char('y'), Confirm::ExitFinished(sessions)) => Step::sending(
                    "exit-finished",
                    Command::SessionEndFinished {
                        only: Some(sessions),
                    },
                ),
                _ => Step::used("confirm-cancel"),
            };
        }
        // While the owner types into the pane, every character is text, `?` and `L` too.
        let is_typing = self.focus == Focus::Pane && self.pane.typing().is_some();
        match key {
            _ if is_typing => {}
            Key::Tab => return self.switch_focus(now),
            Key::Char('?') => return self.open(Overlay::Help),
            Key::Char('L') => return self.open(Overlay::Log),
            _ => {}
        }
        if self.focus() == Focus::Pane {
            return match self.pane.key(key, now) {
                PaneOutcome::IgnoredInGrace => Step::used("approval-ignored-in-grace"),
                PaneOutcome::Handled { used, send } => Step {
                    used: Some(used),
                    send: send.into_iter().collect(),
                },
                PaneOutcome::NotMine => Step::used("unbound"),
            };
        }
        self.list_key(key)
    }

    fn list_key(&mut self, key: Key) -> Step {
        match key {
            Key::Char('j') | Key::Down => {
                self.move_selection(1);
                Step::used("move")
            }
            Key::Char('k') | Key::Up => {
                self.move_selection(-1);
                Step::used("move")
            }
            Key::Enter => match self.selected.clone() {
                Some(session) => Step::sending("open", Command::SessionShow { session }),
                None => Step::used("open"),
            },
            Key::Char('n') => match self.next_needing_you() {
                Some(session) => {
                    self.selected = Some(session.clone());
                    Step::sending("next-needing-you", Command::SessionShow { session })
                }
                None => {
                    self.note("nothing needs you".to_owned());
                    Step::used("next-needing-you")
                }
            },
            Key::Char('N') => {
                let roots = self
                    .board
                    .projects()
                    .iter()
                    .map(|project| project.root.clone())
                    .collect();
                self.form = Some(Form::new(roots));
                Step::sending("new-session-form", Command::ProviderList)
            }
            Key::Char('F') => {
                let (workflows, command) = Workflows::open();
                self.workflows = Some(workflows);
                Step::sending("workflows", command)
            }
            Key::Char('r') => self.ask_to_confirm(Confirm::Restart, "restart-ask"),
            Key::Char('x') => self.ask_to_confirm(Confirm::Exit, "exit-ask"),
            Key::Char('X') => {
                let finished: Vec<String> = self
                    .board
                    .finished_sessions()
                    .into_iter()
                    .map(|session| session.id.clone())
                    .collect();
                if !finished.is_empty() {
                    self.confirm = Some(Confirm::ExitFinished(finished));
                }
                Step::used("exit-finished-ask")
            }
            _ => Step::used("unbound"),
        }
    }

    fn ask_to_confirm(&mut self, confirm: fn(String) -> Confirm, used: &'static str) -> Step {
        if let Some(session) = self.selected.clone() {
            self.confirm = Some(confirm(session));
        }
        Step::used(used)
    }

    fn open(&mut self, overlay: Overlay) -> Step {
        self.overlay = Some(overlay);
        Step::used(match overlay {
            Overlay::Help => "help",
            Overlay::Log => "log-view",
        })
    }

    fn switch_focus(&mut self, now: Instant) -> Step {
        if self.focus() == Focus::Pane || self.pane.shown().is_none() {
            self.focus = Focus::List;
            return Step::used("focus-list");
        }
        self.focus = Focus::Pane;
        self.pane.focus(now);
        Step::used("focus-pane")
    }

    fn move_selection(&mut self, step: isize) {
        let listed = self.board.listed_sessions();
        if listed.is_empty() {
            self.selected = None;
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|selected| listed.iter().position(|session| session.id == *selected));
        let next = match at {
            None => 0,
            Some(at) => at.saturating_add_signed(step).min(listed.len() - 1),
        };
        self.selected = Some(listed[next].id.clone());
    }

    /// The first session after the selected one, in list order and wrapping around, that
    /// needs the owner.
    fn next_needing_you(&self) -> Option<String> {
        let listed = self.board.listed_sessions();
        let start = self
            .selected
            .as_ref()
            .and_then(|selected| listed.iter().position(|session| session.id == *selected))
            .map_or(0, |at| at + 1);
        (0..listed.len())
            .map(|offset| listed[(start + offset) % listed.len()])
            .find(|session| self.board.needs_you(&session.id))
            .map(|session| session.id.clone())
    }

    /// The selection follows the sessions: an unknown or ended one moves to the first.
    fn keep_selection(&mut self) {
        let listed = self.board.listed_sessions();
        let still_listed = self
            .selected
            .as_ref()
            .is_some_and(|selected| listed.iter().any(|session| session.id == *selected));
        if !still_listed {
            self.selected = listed.first().map(|session| session.id.clone());
        }
    }

    /// The daemon's answer to `command`, which the runtime sent for this app; returns what
    /// to send the daemon next.
    pub fn answer(
        &mut self,
        command: &Command,
        answer: Result<Reply, String>,
        now: Instant,
    ) -> Option<Command> {
        let reply = match answer {
            Ok(reply) => reply,
            Err(problem) => {
                self.failed(command, problem, now);
                return None;
            }
        };
        let mut send = None;
        if self
            .failure
            .as_ref()
            .is_some_and(|failure| failure.kind == command_label(command))
        {
            self.failure = None;
        }
        if let Some(workflows) = &mut self.workflows
            && workflows.answer(command, &reply)
        {
            if let Reply::Workflow { workflow } = &reply
                && !matches!(command, Command::WorkflowShow { .. })
            {
                self.note(format!(
                    "workflow {} is {}",
                    workflow.id,
                    workflow.status.as_str()
                ));
            }
            return None;
        }
        match reply {
            Reply::Workflows { .. } | Reply::Workflow { .. } | Reply::WorkflowPacket { .. } => {
                // The answer to a workflow view closed since.
            }
            Reply::Projects { projects } => self.board.set_projects(projects),
            Reply::AgentReports { reports } => self.board.set_reports(reports),
            Reply::ApprovalQueue { runs, guides } => {
                self.board.set_queue(runs, guides);
                send = self.follow_queue(now);
            }
            Reply::ManagerFocused { took_from_viewer } => {
                send = self.keys_moved(command, took_from_viewer, now);
            }
            Reply::Providers { providers } => {
                if let Some(form) = &mut self.form {
                    form.set_agents(providers);
                }
            }
            Reply::Session { session } => {
                let what = match command {
                    Command::SessionNew { .. } => {
                        // Only the form that sent it closes: one opened since stays.
                        if self
                            .form
                            .as_ref()
                            .is_some_and(|form| form.submitted.as_ref() == Some(command))
                        {
                            self.form = None;
                        }
                        self.selected = Some(session.id.clone());
                        "started"
                    }
                    Command::SessionShow { .. } => "opened",
                    Command::SessionRestart { .. } => "restarted",
                    Command::SessionEnd { .. } => "exited",
                    _ => "changed",
                };
                self.note(format!("{what} {} in {}", session.id, session.working_dir));
                self.board.put_session(*session);
                self.failure = None;
            }
            Reply::EndedFinished { ended, failed } => {
                self.note(format!("exited {} finished sessions", ended.len()));
                for session in ended {
                    self.board.put_session(session);
                }
                for failure in failed {
                    self.note(format!(
                        "{} not exited: {}",
                        failure.session, failure.error.message
                    ));
                }
            }
            Reply::RunRequest { request } => {
                self.note(format!("request {} is {:?}", request.id, request.state));
                self.failure = None;
            }
            Reply::Guide { guide } => {
                self.note(format!("guide {} is {:?}", guide.id, guide.state));
                self.failure = None;
            }
            other => self.note(format!("unexpected answer {other:?}")),
        }
        self.keep_selection();
        send
    }

    fn failed(&mut self, command: &Command, problem: String, now: Instant) {
        if is_column_focus(command) {
            self.pane.keys_answered(now);
            if self.taken_keys == TakenKeys::Asking {
                self.taken_keys = TakenKeys::NotTaken;
            }
        }
        if let Some(workflows) = &mut self.workflows {
            workflows.failed(command, &problem);
        }
        if let Some(form) = &mut self.form
            && form.submitted.as_ref() == Some(command)
        {
            form.submitted = None;
            form.problem = Some(problem.clone());
        }
        let kind = command_label(command);
        let is_logged = self.feed_problem.as_ref() == Some(&problem)
            || self
                .failure
                .as_ref()
                .is_some_and(|failure| failure.problem == problem);
        if !is_logged {
            self.note(format!("{kind}: {problem}"));
        }
        self.failure = Some(Failure { kind, problem });
    }

    /// The pane follows the queue; it takes focus when it shows a request and gives it back
    /// to the list when nothing waits. Taking focus also asks the daemon for tmux's keys,
    /// which may be in the viewer, where the owner was typing to an agent (#317); once
    /// nothing waits, keys taken from the viewer go back to it (#345).
    fn follow_queue(&mut self, now: Instant) -> Option<Command> {
        let was_showing = self.pane.shown().is_some();
        self.pane.sync(&self.board, now);
        match (was_showing, self.pane.shown().is_some()) {
            (false, true) => {
                self.focus = Focus::Pane;
                self.pane.ask_for_keys();
                if self.taken_keys == TakenKeys::NotTaken {
                    self.taken_keys = TakenKeys::Asking;
                }
                Some(Command::ManagerFocus {
                    pane: ManagerPaneView::Column,
                })
            }
            (_, false) => {
                // The owner left the pane for the list, the help or the log: they use the
                // column, so its keys stay.
                let was_in_pane = self.focus == Focus::Pane;
                self.focus = Focus::List;
                if !was_in_pane || self.overlay.is_some() {
                    self.keep_keys();
                }
                self.give_keys_back()
            }
            _ => None,
        }
    }

    /// tmux moved the keys `command` asked for. A request's keys came perhaps long after the
    /// pane showed it, and the pane offered no key until then: the first key they bring
    /// waits out the grace again (#439).
    fn keys_moved(
        &mut self,
        command: &Command,
        took_from_viewer: bool,
        now: Instant,
    ) -> Option<Command> {
        if !is_column_focus(command) {
            return None;
        }
        if took_from_viewer {
            self.taken_keys = TakenKeys::FromViewer;
        } else if self.taken_keys == TakenKeys::Asking {
            self.taken_keys = TakenKeys::NotTaken;
        }
        self.pane.keys_answered(now);
        if self.pane.shown().is_some() {
            return None;
        }
        // The queue emptied while tmux moved them.
        self.give_keys_back()
    }

    /// Keys the owner now uses in the column are theirs: none go back to the viewer.
    fn keep_keys(&mut self) {
        if self.taken_keys == TakenKeys::FromViewer {
            self.taken_keys = TakenKeys::NotTaken;
        }
    }

    /// Keys taken from the viewer go back to it, unless the owner is typing in the column:
    /// a form, a confirmation, the workflow view or an overlay is open.
    fn give_keys_back(&mut self) -> Option<Command> {
        let is_owner_in_column = self.form.is_some()
            || self.confirm.is_some()
            || self.workflows.is_some()
            || self.overlay.is_some();
        if is_owner_in_column {
            self.keep_keys();
        }
        if self.taken_keys != TakenKeys::FromViewer {
            return None;
        }
        self.taken_keys = TakenKeys::NotTaken;
        Some(Command::ManagerFocus {
            pane: ManagerPaneView::Viewer,
        })
    }

    fn note(&mut self, entry: String) {
        if self.log.len() == LOG_ENTRIES {
            self.log.pop_front();
        }
        self.log.push_back(entry);
    }
}

fn is_column_focus(command: &Command) -> bool {
    *command
        == Command::ManagerFocus {
            pane: ManagerPaneView::Column,
        }
}

/// The command's wire kind, for the status line and the log.
fn command_label(command: &Command) -> String {
    serde_json::to_value(command)
        .ok()
        .and_then(|value| value["kind"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "command".to_owned())
}
