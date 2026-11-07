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
    /// The provider's request failed; `reason` is its own word for why, when it gives one.
    Failed {
        reason: Option<String>,
    },
    /// The plan's usage limit stopped the turn.
    QuotaStopped,
}

impl TurnOutcome {
    /// The outcome's name, as the agent channel's `--outcome` and its stored column spell it.
    pub fn name(&self) -> &'static str {
        match self {
            TurnOutcome::Completed => "completed",
            TurnOutcome::Failed { .. } => "failed",
            TurnOutcome::QuotaStopped => "quota-stopped",
        }
    }

    /// The outcome `name` spells, with `reason` for a failure; `None` for another name.
    pub fn from_name(name: &str, reason: Option<String>) -> Option<TurnOutcome> {
        match name {
            "completed" => Some(TurnOutcome::Completed),
            "failed" => Some(TurnOutcome::Failed { reason }),
            "quota-stopped" => Some(TurnOutcome::QuotaStopped),
            _ => None,
        }
    }

    /// A failure's reason, when it gives one.
    pub fn failure_reason(&self) -> Option<&str> {
        match self {
            TurnOutcome::Failed { reason } => reason.as_deref(),
            _ => None,
        }
    }
}

/// What triggered a compaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTrigger {
    /// The harness compacted by itself.
    Auto,
    /// Someone asked for it, as with `/compact`.
    Manual,
}

impl CompactionTrigger {
    /// The trigger's name, as the agent channel's `--compaction-trigger` and its stored
    /// column spell it.
    pub fn name(self) -> &'static str {
        match self {
            CompactionTrigger::Auto => "auto",
            CompactionTrigger::Manual => "manual",
        }
    }

    /// The trigger `name` spells; `None` for another name.
    pub fn from_name(name: &str) -> Option<CompactionTrigger> {
        [CompactionTrigger::Auto, CompactionTrigger::Manual]
            .into_iter()
            .find(|trigger| trigger.name() == name)
    }
}

/// Which moment of a context compaction a harness announced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionPhase {
    Started,
    Ended,
    Failed,
}

impl CompactionPhase {
    /// The phase's name, as the agent channel's `--compaction` spells it.
    pub fn name(self) -> &'static str {
        match self {
            CompactionPhase::Started => "started",
            CompactionPhase::Ended => "ended",
            CompactionPhase::Failed => "failed",
        }
    }
}

/// One moment of a compaction with what the harness said about it. Sizes are the harness's
/// own token counts of the context, never converted from bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionMoment {
    pub phase: CompactionPhase,
    pub trigger: Option<CompactionTrigger>,
    pub tokens_before: Option<u64>,
    pub tokens_after: Option<u64>,
}

impl CompactionMoment {
    /// The `cockpit` arguments that report this moment on the agent channel: `agent report
    /// --compaction PHASE`, then the trigger and the sizes the harness gave. The adapter adds
    /// `--conversation`.
    pub fn report_arguments(&self) -> Vec<String> {
        let mut arguments: Vec<String> = ["agent", "report", "--compaction", self.phase.name()]
            .into_iter()
            .map(str::to_owned)
            .collect();
        if let Some(trigger) = self.trigger {
            arguments.extend(["--compaction-trigger".to_owned(), trigger.name().to_owned()]);
        }
        for (flag, tokens) in [
            ("--compaction-tokens-before", self.tokens_before),
            ("--compaction-tokens-after", self.tokens_after),
        ] {
            if let Some(tokens) = tokens {
                arguments.extend([flag.to_owned(), tokens.to_string()]);
            }
        }
        arguments
    }
}

/// The harness's own context usage at a turn's end (#479), in its own tokens, never converted
/// from bytes: what its last model call was given. `window` is the model's context window,
/// when the harness says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsage {
    pub tokens: u64,
    pub window: Option<u64>,
}

impl ContextUsage {
    /// The arguments that add it to a turn's end report on the agent channel:
    /// `--context-tokens N`, then `--context-window N` when the harness gave one.
    pub fn report_arguments(&self) -> Vec<String> {
        let mut arguments = vec!["--context-tokens".to_owned(), self.tokens.to_string()];
        if let Some(window) = self.window {
            arguments.extend(["--context-window".to_owned(), window.to_string()]);
        }
        arguments
    }
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
    /// A moment of a context compaction. It proves no activity: a compaction can run in the
    /// middle of a turn (#782).
    Compaction(CompactionMoment),
    /// The context a compaction produced started, with the launch brief re-added. Like the
    /// compaction it follows, it proves no activity: it is no wait (#782).
    ContextCompacted,
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
            EventKind::Ended
            | EventKind::Compaction(_)
            | EventKind::ContextCompacted
            | EventKind::Unknown => return None,
        };
        Some(Observation {
            activity,
            observed_by: ObservedBy::Event,
        })
    }
}
