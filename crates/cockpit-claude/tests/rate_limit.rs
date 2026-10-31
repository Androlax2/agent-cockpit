//! A `StopFailure` with `error: "rate_limit"` (#212): Claude Code sends it both when the
//! plan's window is spent and for a transient 429, so the turn is `failed` with reason
//! `rate_limit`, never `quota-stopped`, which would restart the session on another account.

mod common;

use cockpit_claude::hooks;
use cockpit_provider_api::{EventKind, TurnOutcome};

const RATE_LIMIT_FIXTURES: [&str; 2] = [
    "stop-failure-rate-limit-plan-window",
    "stop-failure-rate-limit-transient",
];

#[test]
fn a_rate_limited_turn_fails_with_reason_rate_limit() {
    for fixture in RATE_LIMIT_FIXTURES {
        assert_eq!(
            hooks::normalize(&common::fixture(fixture)).unwrap().kind,
            EventKind::TurnEnded {
                outcome: TurnOutcome::Failed {
                    reason: Some("rate_limit".to_owned())
                }
            },
            "{fixture}"
        );
    }
}

#[test]
fn a_rate_limited_turn_reports_failed_on_the_agent_channel() {
    for fixture in RATE_LIMIT_FIXTURES {
        let event = hooks::normalize(&common::fixture(fixture)).unwrap();

        assert_eq!(
            hooks::report_arguments(&event).unwrap(),
            [
                "agent",
                "report",
                "--state",
                "waiting",
                "--outcome",
                "failed",
                "--outcome-reason",
                "rate_limit",
            ],
            "{fixture}"
        );
    }
}
