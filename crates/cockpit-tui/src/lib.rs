#![doc = include_str!("../README.md")]

mod app;
mod board;
mod feed;
mod form;
mod pane;
mod render;
mod workflows;

pub use app::{App, Confirm, Focus, Key, Overlay, Step};
pub use board::{Board, FeedError, ProjectGroup};
pub use feed::{FeedEvent, follow_feed};
pub use form::{Field, Form, FormOutcome, Place};
pub use pane::{INPUT_GRACE, Pane, PaneOutcome, Shown, Typing};
pub use render::{HELP, draw};
pub use workflows::{HandoffEntry, WorkflowConfirm, Workflows, WorkflowsOutcome};

/// Counts the command `step` ran in the TUI's log (decision 17): its name only, never what
/// was typed.
pub fn record_usage(step: &Step) {
    if let Some(used) = step.used {
        tracing::info!(command = used, "tui command");
    }
}
