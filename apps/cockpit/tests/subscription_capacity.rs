use cockpit::usage::{SubscriptionCapacity, subscription_capacity_of};
use serde_json::{Value, json};

const NOW: i64 = 1_800_000_000;

fn claude(used: f64) -> Value {
    json!({"routing_profile":{"organization":{"has_extra_usage_enabled":false}},
           "five_hour": {"utilization": used, "resets_at": "2027-01-15T09:00:00Z"},
           "seven_day": {"utilization": 20, "resets_at": "2027-01-20T09:00:00Z"}})
}

#[test]
fn routing_uses_raw_percentages_not_the_rounded_display() {
    for (used, expected) in [
        (99.6, SubscriptionCapacity::Available),
        (100.0, SubscriptionCapacity::Exhausted),
    ] {
        assert_eq!(
            subscription_capacity_of(
                "claude",
                "claude-sonnet-4-6",
                &claude(used),
                NOW,
                None,
                NOW,
                true
            ),
            expected
        );
    }
}

#[test]
fn unrelated_model_exhaustion_does_not_block_the_selected_model() {
    let mut usage = claude(30.0);
    usage["seven_day_opus"] = json!({"utilization": 100, "resets_at": "2027-01-20T09:00:00Z"});
    for (model, expected) in [
        ("claude-sonnet-4-6", SubscriptionCapacity::Available),
        ("claude-opus-4-7", SubscriptionCapacity::Exhausted),
    ] {
        assert_eq!(
            subscription_capacity_of("claude", model, &usage, NOW, None, NOW, true),
            expected
        );
    }
}

#[test]
fn native_claude_capacity_requires_confirmed_disabled_extra_usage() {
    for enabled in [json!(true), Value::Null, json!("false")] {
        let mut usage = claude(10.0);
        usage["routing_profile"]["organization"]["has_extra_usage_enabled"] = enabled;
        assert_eq!(
            subscription_capacity_of("claude", "claude-sonnet-4-6", &usage, NOW, None, NOW, true),
            SubscriptionCapacity::Unknown
        );
        usage["five_hour"]["utilization"] = json!(100);
        assert_eq!(
            subscription_capacity_of("claude", "claude-sonnet-4-6", &usage, NOW, None, NOW, true),
            SubscriptionCapacity::Exhausted
        );
    }
}

#[test]
fn capacity_is_unknown_without_fresh_successful_account_matched_evidence() {
    for (fetched, failed, matched) in [
        (NOW - 601, None, true),
        (NOW + 1, None, true),
        (NOW, Some(NOW), true),
        (NOW, None, false),
    ] {
        assert_eq!(
            subscription_capacity_of(
                "claude",
                "claude-sonnet-4-6",
                &claude(10.0),
                fetched,
                failed,
                NOW,
                matched
            ),
            SubscriptionCapacity::Unknown
        );
    }
}

#[test]
fn passed_reset_does_not_imply_recovery() {
    let mut usage = claude(100.0);
    usage["five_hour"]["resets_at"] = json!("2020-01-01T00:00:00Z");
    assert_eq!(
        subscription_capacity_of("claude", "claude-sonnet-4-6", &usage, NOW, None, NOW, true),
        SubscriptionCapacity::Unknown
    );
}

#[test]
fn invalid_missing_or_unknown_scoped_limits_do_not_grant_capacity() {
    for usage in [
        json!({}),
        json!({"five_hour": {"utilization": -1}}),
        json!({"limits": [{"kind": "weekly_scoped", "percent": 10, "scope": {"model": {"display_name": "Unknown"}},
                                     "resets_at": "2027-01-20T09:00:00Z"}]}),
        json!({"five_hour": {"utilization": 0, "resets_at": "not-a-date"}}),
    ] {
        assert_eq!(
            subscription_capacity_of("claude", "claude-sonnet-4-6", &usage, NOW, None, NOW, true),
            SubscriptionCapacity::Unknown
        );
    }
}

#[test]
fn codex_requires_backend_confirmation_of_included_usage_not_credits() {
    for (allowed, expected) in [
        (Some(true), SubscriptionCapacity::Available),
        (Some(false), SubscriptionCapacity::Exhausted),
        (None, SubscriptionCapacity::Unknown),
    ] {
        let usage = json!({"ordinary_usage_allowed": allowed, "rate_limit": {"primary_window": {
            "used_percent": 20, "reset_at": NOW + 3600, "limit_window_seconds": 18000}}});
        assert_eq!(
            subscription_capacity_of("codex", "gpt-6.1-sol", &usage, NOW, None, NOW, true),
            expected
        );
    }
}

#[test]
fn unknown_provider_is_not_assumed_to_have_a_subscription() {
    assert_eq!(
        subscription_capacity_of("other", "unknown", &claude(0.0), NOW, None, NOW, true),
        SubscriptionCapacity::Unknown
    );
}

#[test]
fn malformed_new_limits_cannot_be_hidden_by_legacy_windows() {
    let mut usage = claude(10.0);
    usage["limits"] = json!({"percent": 100});
    assert_eq!(
        subscription_capacity_of("claude", "claude-sonnet-4-6", &usage, NOW, None, NOW, true),
        SubscriptionCapacity::Unknown
    );
}

#[test]
fn native_routing_matches_the_shared_provider_contract_cases() {
    let cases: Value = serde_json::from_str(include_str!(
        "fixtures/subscription-fallback/capacity-cases.json"
    ))
    .unwrap();
    for case in cases.as_array().unwrap() {
        let mut usage = case["usage"].clone();
        if case["provider"] == "anthropic" {
            usage["routing_profile"] = json!({"organization":{"has_extra_usage_enabled":false}});
        }
        if case["provider"] == "openai" {
            usage["ordinary_usage_allowed"] = case["usage"]["rate_limit"]["allowed"].clone();
        }
        let actual = subscription_capacity_of(
            case["provider"].as_str().unwrap(),
            case["model"].as_str().unwrap(),
            &usage,
            NOW,
            None,
            NOW,
            true,
        );
        let expected = match case["expected"].as_str().unwrap() {
            "available" => SubscriptionCapacity::Available,
            "exhausted" => SubscriptionCapacity::Exhausted,
            "unknown" => SubscriptionCapacity::Unknown,
            _ => panic!("invalid fixture expectation"),
        };
        assert_eq!(actual, expected, "{}", case["name"]);
    }
}
