//! `usage-quota` (report 18, #58): the request names nothing, the reply carries each account's
//! plan windows.
use cockpit_protocol::{
    Command, QuotaAccountView, QuotaFailureView, QuotaWindowStateView, QuotaWindowView, Reply,
    decode_request,
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
