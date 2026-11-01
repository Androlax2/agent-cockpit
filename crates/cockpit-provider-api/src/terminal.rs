//! Actions Cockpit takes in a running conversation: as the keystrokes its terminal receives,
//! which the provider decides and the caller sends to the agent's pane, or performed by the
//! provider itself ([`crate::Provider::deliver`]).

use crate::Capability;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action<'message> {
    /// Send the owner's message.
    Submit(&'message str),
    /// Interrupt the turn under way.
    Cancel,
    /// Compact the conversation's context.
    Compact,
}

impl Action<'_> {
    /// The capability a provider must report to take this action.
    pub fn capability(&self) -> Capability {
        match self {
            Action::Submit(_) => Capability::Submit,
            Action::Cancel => Capability::Cancel,
            Action::Compact => Capability::Compact,
        }
    }
}

/// One step of terminal input, sent in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalInput {
    /// Typed literally; never interpreted as key names.
    Text(String),
    Key(Key),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    /// The control chord of a lower-case ASCII letter, such as Ctrl-X.
    Control(char),
}

/// What a provider's delivery of an action did ([`crate::Provider::deliver`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivered {
    /// The harness accepted the message, possibly queued behind a turn. It is not proof
    /// that a turn started or finished.
    Accepted,
    /// This adapter has no direct transport for the action. Nothing was sent; the caller
    /// may ask for terminal input. A failed or uncertain delivery must return an error.
    Unsupported,
    /// The message started a turn: the agent was not working.
    StartedTurn,
    /// The message joined the turn under way.
    JoinedTurn,
    /// The turn under way was interrupted.
    Interrupted,
}
