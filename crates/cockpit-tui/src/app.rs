//! The TUI's state machine: keys and the daemon's answers in, commands out. It never waits:
//! the runtime sends the commands and hands back each answer, so a slow daemon leaves the
//! list, the pane and the form responsive.

use std::collections::VecDeque;
use std::time::Instant;

use cockpit_protocol::{ChangeBatch, Command, Reply};

use crate::board::{Board, FeedError};
use crate::form::{Form, FormOutcome};
use crate::pane::{Pane, PaneOutcome};

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

/// A change to a session the owner must confirm with `y`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Restart(String),
    Exit(String),
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
    pub(crate) overlay: Option<Overlay>,
    pub(crate) confirm: Option<Confirm>,
    pub(crate) log: VecDeque<String>,
    /// The last failure, shown on the status line until the next one or a success.
    pub(crate) status: Option<String>,
}

impl App {
    pub fn new() -> App {
        App::default()
    }

    /// What the runtime asks the daemon on every refresh, until the change feed serves
    /// sessions and checkouts (#41).
    pub fn refresh_commands() -> Vec<Command> {
        vec![
            Command::ProjectList,
            Command::CheckoutList { project: None },
            Command::SessionList { ended: false },
            Command::AgentReports,
            Command::ApprovalQueue,
        ]
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
        self.keep_selection();
        Ok(())
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
        if let Some(confirm) = self.confirm.take() {
            return match (key, confirm) {
                (Key::Char('y'), Confirm::Restart(session)) => {
                    Step::sending("restart", Command::SessionRestart { session })
                }
                (Key::Char('y'), Confirm::Exit(session)) => {
                    Step::sending("exit", Command::SessionEnd { session })
                }
                _ => Step::used("confirm-cancel"),
            };
        }
        match key {
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
            Key::Char('r') => self.ask_to_confirm(Confirm::Restart, "restart-ask"),
            Key::Char('x') => self.ask_to_confirm(Confirm::Exit, "exit-ask"),
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
        let listed: Vec<String> = self
            .board
            .listed_sessions()
            .iter()
            .map(|session| session.id.clone())
            .collect();
        if listed.is_empty() {
            self.selected = None;
            return;
        }
        let at = self
            .selected
            .as_ref()
            .and_then(|selected| listed.iter().position(|id| id == selected));
        let next = match at {
            None => 0,
            Some(at) => at.saturating_add_signed(step).min(listed.len() - 1),
        };
        self.selected = Some(listed[next].clone());
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

    /// The daemon's answer to `command`, which the runtime sent for this app.
    pub fn answer(&mut self, command: &Command, answer: Result<Reply, String>, now: Instant) {
        let reply = match answer {
            Ok(reply) => reply,
            Err(problem) => return self.failed(command, problem),
        };
        match reply {
            Reply::Projects { projects } => self.board.set_projects(projects),
            Reply::Checkouts { checkouts } => self.board.set_checkouts(checkouts),
            Reply::Sessions { sessions } => self.board.set_sessions(sessions),
            Reply::AgentReports { reports } => self.board.set_reports(reports),
            Reply::ApprovalQueue { runs, guides } => {
                self.board.set_queue(runs, guides);
                self.follow_queue(now);
            }
            Reply::Providers { providers } => {
                if let Some(form) = &mut self.form {
                    form.set_agents(providers);
                }
            }
            Reply::Session { session } => {
                let what = match command {
                    Command::SessionNew { .. } => {
                        self.form = None;
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
                self.status = None;
            }
            Reply::RunRequest { request } => {
                self.note(format!("request {} is {:?}", request.id, request.state));
                self.status = None;
            }
            Reply::Guide { guide } => {
                self.note(format!("guide {} is {:?}", guide.id, guide.state));
                self.status = None;
            }
            other => self.note(format!("unexpected answer {other:?}")),
        }
        self.keep_selection();
    }

    fn failed(&mut self, command: &Command, problem: String) {
        let text = format!("{}: {problem}", command_label(command));
        if let (Command::SessionNew { .. }, Some(form)) = (command, &mut self.form) {
            form.problem = Some(problem);
        }
        if self.status.as_deref() != Some(text.as_str()) {
            self.note(text.clone());
        }
        self.status = Some(text);
    }

    /// The pane follows the queue; it takes focus when it shows a request and gives it back
    /// to the list when nothing waits.
    fn follow_queue(&mut self, now: Instant) {
        let was_showing = self.pane.shown().is_some();
        self.pane.sync(&self.board, now);
        match (was_showing, self.pane.shown().is_some()) {
            (false, true) => self.focus = Focus::Pane,
            (_, false) => self.focus = Focus::List,
            _ => {}
        }
    }

    fn note(&mut self, entry: String) {
        if self.log.len() == LOG_ENTRIES {
            self.log.pop_front();
        }
        self.log.push_back(entry);
    }
}

/// The command's wire kind, for the status line and the log.
fn command_label(command: &Command) -> String {
    serde_json::to_value(command)
        .ok()
        .and_then(|value| value["kind"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "command".to_owned())
}
