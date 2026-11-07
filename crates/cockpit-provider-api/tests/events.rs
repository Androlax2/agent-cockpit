//! The normalized event vocabulary: what each event says about the agent's activity. Events
//! are observed from structured sources, so their observation is never a screen guess.

use cockpit_provider_api::{
    Activity, CompactionMoment, CompactionPhase, CompactionTrigger, ContextUsage, ConversationId,
    EventKind, Observation, ObservedBy, ProviderEvent, TurnOutcome,
};

fn event(kind: EventKind) -> ProviderEvent {
    ProviderEvent {
        kind,
        name: "Raw".to_owned(),
        raw: "{}".to_owned(),
    }
}

fn observed(activity: Activity) -> Option<Observation> {
    Some(Observation {
        activity,
        observed_by: ObservedBy::Event,
    })
}

#[test]
fn each_event_kind_names_the_activity_it_proves() {
    for (kind, expected) in [
        (
            EventKind::Started {
                conversation: Some(ConversationId::new("c-1").unwrap()),
            },
            observed(Activity::Waiting),
        ),
        (EventKind::PromptSubmitted, observed(Activity::Working)),
        (
            EventKind::ToolUsed {
                tool: "Bash".to_owned(),
            },
            observed(Activity::Working),
        ),
        (
            EventKind::NeedsYou { question: None },
            observed(Activity::NeedsYou),
        ),
        (EventKind::Idle, observed(Activity::Waiting)),
        (
            EventKind::TurnEnded {
                outcome: TurnOutcome::QuotaStopped,
            },
            observed(Activity::Waiting),
        ),
        (EventKind::Ended, None),
        (EventKind::ContextCompacted, None),
        (
            EventKind::Compaction(CompactionMoment {
                phase: CompactionPhase::Started,
                trigger: Some(CompactionTrigger::Auto),
                tokens_before: None,
                tokens_after: None,
            }),
            None,
        ),
        (EventKind::Unknown, None),
    ] {
        assert_eq!(event(kind.clone()).observation(), expected, "{kind:?}");
    }
}

#[test]
fn a_context_usage_reports_its_tokens_and_the_window_when_the_harness_gave_one() {
    for (usage, expected) in [
        (
            ContextUsage {
                tokens: 45_123,
                window: Some(258_400),
            },
            vec!["--context-tokens", "45123", "--context-window", "258400"],
        ),
        (
            ContextUsage {
                tokens: 45_123,
                window: None,
            },
            vec!["--context-tokens", "45123"],
        ),
    ] {
        assert_eq!(usage.report_arguments(), expected, "{usage:?}");
    }
}
