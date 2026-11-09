//! `usage-quota` (report 18, #58): the request names nothing, the reply carries each account's
//! plan windows. A `quota-alert` entity on the change feed carries the alert a window fired.
use cockpit_protocol::{
    Command, CostArchiveView, CostRowView, DayDigestView, DigestView, IncidentView, ProjectDayView,
    QuotaAccountView, QuotaAlertView, QuotaFailureView, QuotaWindowStateView, QuotaWindowView,
    Reply, decode_request,
};
use serde_json::json;

#[test]
fn the_request_round_trips_through_its_wire_form() {
    let wire = json!({"id": 1, "command": {"kind": "usage-quota"}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(request.command, Command::UsageQuota);
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn the_reply_names_each_accounts_windows_and_its_last_failure() {
    let reply = Reply::UsageQuota {
        accounts: vec![QuotaAccountView {
            provider: "claude".to_owned(),
            account: "default".to_owned(),
            plan: Some("max".to_owned()),
            is_blocked: true,
            observed_at: Some(1_800_000_000),
            is_current: false,
            failure: Some(QuotaFailureView {
                at: 1_800_000_300,
                reason: "the usage endpoint answered HTTP 429".to_owned(),
            }),
            windows: vec![QuotaWindowView {
                limit: "seven-day:sonnet".to_owned(),
                scope: Some("Sonnet".to_owned()),
                length_seconds: Some(604_800),
                used_percent: 81,
                resets_at: Some(1_800_100_000),
                state: QuotaWindowStateView::Stale,
            }],
        }],
    };

    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        json!({"kind": "usage-quota", "accounts": [{
            "provider": "claude",
            "account": "default",
            "plan": "max",
            "is_blocked": true,
            "observed_at": 1_800_000_000,
            "is_current": false,
            "failure": {"at": 1_800_000_300, "reason": "the usage endpoint answered HTTP 429"},
            "windows": [{
                "limit": "seven-day:sonnet",
                "scope": "Sonnet",
                "length_seconds": 604_800,
                "used_percent": 81,
                "resets_at": 1_800_100_000,
                "state": "stale",
            }],
        }]})
    );
}

#[test]
fn a_quota_alert_names_its_window_and_the_threshold_it_passed() {
    let alert = QuotaAlertView {
        provider: "codex".to_owned(),
        account: "default".to_owned(),
        limit: "five-hour".to_owned(),
        scope: None,
        threshold: 95,
        used_percent: 97,
        resets_at: Some(1_800_010_000),
        fired_at: 1_800_000_000,
    };

    let wire = serde_json::to_value(&alert).unwrap();

    assert_eq!(
        wire,
        json!({
            "provider": "codex",
            "account": "default",
            "limit": "five-hour",
            "scope": null,
            "threshold": 95,
            "used_percent": 97,
            "resets_at": 1_800_010_000,
            "fired_at": 1_800_000_000,
        })
    );
    assert_eq!(
        serde_json::from_value::<QuotaAlertView>(wire).unwrap(),
        alert
    );
}

#[test]
fn the_cost_request_names_how_many_days_and_round_trips() {
    let wire = json!({"id": 1, "command": {"kind": "usage-cost", "days": 7}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(request.command, Command::UsageCost { days: 7 });
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn the_cost_reply_carries_each_days_totals_in_whole_nano_dollars() {
    let reply = Reply::UsageCost {
        cost: CostArchiveView {
            first_day: "2026-11-02".to_owned(),
            last_day: "2026-11-02".to_owned(),
            rows: vec![CostRowView {
                day: "2026-11-02".to_owned(),
                provider: "claude".to_owned(),
                account: "work".to_owned(),
                project: "app".to_owned(),
                replies: 3,
                unpriced_replies: 1,
                input_tokens: 10,
                output_tokens: 20,
                cache_read_tokens: 300,
                cache_write_tokens: 45,
                cost_nanos: 740_000,
            }],
        },
    };

    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        json!({"kind": "usage-cost", "cost": {
            "first_day": "2026-11-02",
            "last_day": "2026-11-02",
            "rows": [{
                "day": "2026-11-02",
                "provider": "claude",
                "account": "work",
                "project": "app",
                "replies": 3,
                "unpriced_replies": 1,
                "input_tokens": 10,
                "output_tokens": 20,
                "cache_read_tokens": 300,
                "cache_write_tokens": 45,
                "cost_nanos": 740_000
            }]
        }})
    );
}

#[test]
fn the_digest_request_names_how_many_days_and_round_trips() {
    let wire = json!({"id": 1, "command": {"kind": "usage-digest", "days": 2}});

    let request = decode_request(&wire.to_string()).unwrap();

    assert_eq!(request.command, Command::UsageDigest { days: 2 });
    assert_eq!(serde_json::to_value(request).unwrap(), wire);
}

#[test]
fn the_digest_reply_carries_each_days_figures_projects_and_incidents() {
    let reply = Reply::UsageDigest {
        digest: DigestView {
            first_day: "2026-11-02".to_owned(),
            last_day: "2026-11-02".to_owned(),
            days: vec![DayDigestView {
                day: "2026-11-02".to_owned(),
                sessions: 3,
                turns: 4,
                approvals: 3,
                seconds_with_agent: 3600,
                replies: 4,
                unpriced_replies: 1,
                cost_nanos: 17_000_000,
                projects: vec![ProjectDayView {
                    project: "app".to_owned(),
                    sessions: 2,
                    turns: 3,
                    approvals: 2,
                    seconds_with_agent: 3000,
                    replies: 2,
                    unpriced_replies: 0,
                    cost_nanos: 14_000_000,
                }],
                incidents: vec![IncidentView {
                    provider: "claude".to_owned(),
                    title: "Elevated errors".to_owned(),
                    started_at: 1_793_606_400,
                    resolved_at: None,
                }],
            }],
        },
    };

    assert_eq!(
        serde_json::to_value(&reply).unwrap(),
        json!({"kind": "usage-digest", "digest": {
            "first_day": "2026-11-02",
            "last_day": "2026-11-02",
            "days": [{
                "day": "2026-11-02",
                "sessions": 3,
                "turns": 4,
                "approvals": 3,
                "seconds_with_agent": 3600,
                "replies": 4,
                "unpriced_replies": 1,
                "cost_nanos": 17_000_000,
                "projects": [{
                    "project": "app",
                    "sessions": 2,
                    "turns": 3,
                    "approvals": 2,
                    "seconds_with_agent": 3000,
                    "replies": 2,
                    "unpriced_replies": 0,
                    "cost_nanos": 14_000_000
                }],
                "incidents": [{
                    "provider": "claude",
                    "title": "Elevated errors",
                    "started_at": 1_793_606_400,
                    "resolved_at": null
                }]
            }]
        }})
    );
}
