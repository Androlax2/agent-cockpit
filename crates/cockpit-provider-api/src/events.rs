//! The normalized activity vocabulary every adapter maps its harness's events onto. Terminal
//! bytes are untrusted display data (report 05): an activity read from the screen is a
//! fallback that records its confidence, and task completion never comes from it.

use crate::ConversationId;

/// What the agent is doing, as far as Cockpit can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// In a turn.
    Working,
    /// At its prompt, asking nothing.
    Waiting,
    /// A dialog or question waits for the owner.
    NeedsYou,
}

/// How strongly a screen reading supports its activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// A pattern only that activity draws (a permission dialog, a running spinner).
    High,
    /// A pattern other activities can draw too (the prompt line, visible behind a menu).
    Low,
}

/// Where an activity came from: `observed-by: event|screen`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObservedBy {
    Event,
    Screen(Confidence),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    pub activity: Activity,
    pub observed_by: ObservedBy,
}

/// How a turn ended, when the provider has the `outcome` capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    /// The provider's request failed; `reason` is its own word for why.
    Failed {
        reason: String,
    },
    /// The plan's usage limit stopped the turn.
    QuotaStopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventKind {
    /// The conversation started or was resumed, and waits at its prompt.
    Started {
        conversation: Option<ConversationId>,
    },
    PromptSubmitted,
    ToolUsed {
        tool: String,
    },
    /// A dialog or question waits for the owner; `question` when the provider says it.
    NeedsYou {
        question: Option<String>,
    },
    /// The agent sits at its prompt.
    Idle,
    TurnEnded {
        outcome: TurnOutcome,
    },
    /// The harness says the conversation ended; its process may still be running.
    Ended,
    /// An event the adapter does not normalize; [`ProviderEvent::raw`] keeps it.
    Unknown,
}

/// One event from the harness: normalized, with the raw event kept beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEvent {
    pub kind: EventKind,
    /// The harness's own name for the event.
    pub name: String,
    /// The event as the harness delivered it.
    pub raw: String,
}

impl ProviderEvent {
    /// The activity this event proves, if any.
    pub fn observation(&self) -> Option<Observation> {
        let activity = match &self.kind {
            EventKind::Started { .. } | EventKind::Idle | EventKind::TurnEnded { .. } => {
                Activity::Waiting
            }
            EventKind::PromptSubmitted | EventKind::ToolUsed { .. } => Activity::Working,
            EventKind::NeedsYou { .. } => Activity::NeedsYou,
            EventKind::Ended | EventKind::Unknown => return None,
        };
        Some(Observation {
            activity,
            observed_by: ObservedBy::Event,
        })
    }
}
