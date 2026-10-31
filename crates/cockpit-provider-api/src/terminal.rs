//! Actions Cockpit takes in a running conversation, as the keystrokes its terminal receives.
//! The provider decides the keys; the caller sends them to the agent's pane.

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
