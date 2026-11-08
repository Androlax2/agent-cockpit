//! `usage-quota` (report 18, #58): the request names nothing, the reply carries each account's
//! plan windows. A `quota-alert` entity on the change feed carries the alert a window fired.
use cockpit_protocol::{
    Command, QuotaAccountView, QuotaAlertView, QuotaFailureView, QuotaWindowStateView,
    QuotaWindowView, Reply, decode_request,
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
