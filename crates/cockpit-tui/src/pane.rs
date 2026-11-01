//! The approval pane's input model (#166, decided in #178): which request it shows, when it
//! takes input, and what each key does. Pure: the caller passes the time.
//!
//! - Approving is `y` only: no navigation key (`j`, `k`, arrows, Enter) approves.
//! - Input is ignored for [`INPUT_GRACE`] after the pane shows a request (a new one or a new
//!   revision) or takes focus, and for [`INPUT_GRACE`] after each key it ignored: an owner
//!   still typing to an agent when the pane took tmux's keys approves nothing (#317). The
//!   daemon's own grace refusal is the backstop.
//! - While the owner types into the pane, the request shown stays, whatever arrives.

use std::time::{Duration, Instant};

use cockpit_protocol::{Command, GuideStateView, GuideView, RunRequestView, RunStateView};

use crate::Key;
use crate::board::Board;

/// How long the pane ignores input after it shows a request or takes focus. The daemon
/// refuses an approval until two whole seconds after the revision appeared (between one and
/// two real seconds); the pane shows a revision only after the daemon created it, so two
/// seconds from showing always outlast that, and a `y` after the hint is never refused (#337).
pub const INPUT_GRACE: Duration = Duration::from_secs(2);

/// The request the pane shows, as it was when shown: an approval names this revision and
/// digest, never a newer one the owner has not seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shown {
    Run {
        id: i64,
        revision: i64,
        digest: String,
        waiting_for_input: bool,
    },
    Guide {
        id: i64,
        step: usize,
        asks: bool,
    },
}

/// What the owner is typing into the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typing {
    /// A line for the running command's terminal.
    Input,
    /// The answer a guide step asks for.
    Answer,
    /// Why the owner is stuck on a guide step.
    StuckNote,
}

/// What a key did in the pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneOutcome {
    /// Within the grace: the key did nothing.
    IgnoredInGrace,
    /// The pane took the key; `send` is the decision to send, if any; `used` names the
    /// command for the usage count.
    Handled {
        used: &'static str,
        send: Option<Command>,
    },
    /// Not a key of the pane.
    NotMine,
}

#[derive(Debug, Default, Clone)]
pub struct Pane {
    shown: Option<Shown>,
    accepts_input_at: Option<Instant>,
    typing: Option<Typing>,
    typed: String,
    /// The whole command instead of its folded start.
    expanded: bool,
}

impl Pane {
    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref()
    }

    pub fn typing(&self) -> Option<(Typing, &str)> {
        self.typing.map(|typing| (typing, self.typed.as_str()))
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// Whether a key now would be ignored.
    pub fn in_grace(&self, now: Instant) -> bool {
        self.accepts_input_at.is_some_and(|ready| now < ready)
    }

    /// Follows the queue after it changed: shows its head, unless the owner is typing into
    /// a request that still waits, which then stays.
    pub fn sync(&mut self, board: &Board, now: Instant) {
        if self.typing.is_some()
            && self
                .shown
                .as_ref()
                .is_some_and(|shown| still_waits(shown, board))
        {
            return;
        }
        let head = head_of(board);
        if head != self.shown {
            self.shown = head;
            self.typing = None;
            self.typed.clear();
            self.expanded = false;
            self.accepts_input_at = self.shown.as_ref().map(|_| now + INPUT_GRACE);
        }
    }

    /// The pane took focus: input waits out the grace again.
    pub fn focus(&mut self, now: Instant) {
        if self.shown.is_some() {
            self.accepts_input_at = Some(now + INPUT_GRACE);
        }
    }

    pub fn key(&mut self, key: Key, now: Instant) -> PaneOutcome {
        let Some(shown) = self.shown.clone() else {
            return PaneOutcome::NotMine;
        };
        if self.in_grace(now) {
            self.accepts_input_at = Some(now + INPUT_GRACE);
            return PaneOutcome::IgnoredInGrace;
        }
        if let Some(typing) = self.typing {
            return self.typing_key(&shown, typing, key);
        }
        match (&shown, key) {
            (_, Key::Char('m')) => {
                self.expanded = !self.expanded;
                handled("approval-more", None)
            }
            (
                Shown::Run {
                    waiting_for_input: true,
                    ..
                },
                Key::Char('i'),
            ) => {
                self.typing = Some(Typing::Input);
                handled("approval-type-input", None)
            }
            (
                Shown::Run {
                    id,
                    revision,
                    digest,
                    waiting_for_input: false,
                },
                Key::Char('y'),
            ) => handled(
                "approval-approve",
                Some(Command::ApprovalApprove {
                    request: *id,
                    revision: *revision,
                    digest: digest.clone(),
                }),
            ),
            (
                Shown::Run {
                    id,
                    waiting_for_input: false,
                    ..
                },
                Key::Char('x'),
            ) => handled(
                "approval-decline",
                Some(Command::ApprovalDecline { request: *id }),
            ),
            (Shown::Guide { asks: true, .. }, Key::Char('y')) => {
                self.typing = Some(Typing::Answer);
                handled("guide-type-answer", None)
            }
            (Shown::Guide { id, step, .. }, Key::Char('y')) => handled(
                "guide-done",
                Some(Command::GuideAnswer {
                    guide: *id,
                    step: *step,
                    answer: None,
                }),
            ),
            (Shown::Guide { .. }, Key::Char('s')) => {
                self.typing = Some(Typing::StuckNote);
                handled("guide-type-stuck", None)
            }
            (Shown::Guide { id, .. }, Key::Char('x')) => {
                handled("guide-close", Some(Command::GuideClose { guide: *id }))
            }
            _ => PaneOutcome::NotMine,
        }
    }

    fn typing_key(&mut self, shown: &Shown, typing: Typing, key: Key) -> PaneOutcome {
        match key {
            Key::Char(character) => {
                self.typed.push(character);
                handled("approval-typing", None)
            }
            Key::Backspace => {
                self.typed.pop();
                handled("approval-typing", None)
            }
            Key::Esc => {
                self.typing = None;
                self.typed.clear();
                handled("approval-typing-cancel", None)
            }
            Key::Enter => {
                let text = std::mem::take(&mut self.typed);
                self.typing = None;
                let (used, send) = match (shown, typing) {
                    (Shown::Run { id, .. }, Typing::Input) => (
                        "approval-input",
                        Command::ApprovalInput { request: *id, text },
                    ),
                    (Shown::Guide { id, step, .. }, Typing::Answer) => (
                        "guide-answer",
                        Command::GuideAnswer {
                            guide: *id,
                            step: *step,
                            answer: Some(text),
                        },
                    ),
                    (Shown::Guide { id, step, .. }, Typing::StuckNote) => (
                        "guide-stuck",
                        Command::GuideStuck {
                            guide: *id,
                            step: *step,
                            note: text,
                        },
                    ),
                    _ => return handled("approval-typing", None),
                };
                handled(used, Some(send))
            }
            _ => PaneOutcome::NotMine,
        }
    }
}

fn handled(used: &'static str, send: Option<Command>) -> PaneOutcome {
    PaneOutcome::Handled { used, send }
}

/// What waits on the owner, in the daemon's queue order (a command waiting for input, then
/// asking requests oldest first), then guides.
fn waiting(board: &Board) -> impl Iterator<Item = Shown> + '_ {
    board
        .runs
        .iter()
        .filter(|run| run.state == RunStateView::Asking || waits_for_input(run))
        .map(shown_run)
        .chain(
            board
                .guides
                .iter()
                .filter(|guide| guide.state == GuideStateView::Asking)
                .map(shown_guide),
        )
}

fn head_of(board: &Board) -> Option<Shown> {
    waiting(board).next()
}

fn still_waits(shown: &Shown, board: &Board) -> bool {
    waiting(board).any(|waiting| waiting == *shown)
}

/// A running command that waits for a line from the owner.
pub(crate) fn waits_for_input(run: &RunRequestView) -> bool {
    run.state == RunStateView::Running && run.waiting_for_input
}

fn shown_run(run: &RunRequestView) -> Shown {
    Shown::Run {
        id: run.id,
        revision: run.revision,
        digest: run.digest.clone(),
        waiting_for_input: waits_for_input(run),
    }
}

fn shown_guide(guide: &GuideView) -> Shown {
    Shown::Guide {
        id: guide.id,
        step: guide.current_step,
        asks: guide
            .steps
            .get(guide.current_step)
            .is_some_and(|step| step.ask.is_some()),
    }
}
