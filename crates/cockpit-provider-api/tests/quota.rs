//! The quota facts an adapter hands `cockpit-usage` (report 18), validated where they are made.

use cockpit_provider_api::{LimitKey, MAX_WINDOWS, QuotaReading, QuotaWindow};

fn window(limit: &str, used_percent: f64) -> QuotaWindow {
    QuotaWindow::new(
        LimitKey::new(limit).unwrap(),
        None,
        Some(18_000),
        used_percent,
        Some(1_800_000_000),
    )
    .unwrap()
}

#[test]
fn a_limit_key_names_a_window_and_its_scope() {
    for key in [
        "five-hour",
        "seven-day",
        "seven-day:sonnet",
        "codex:primary2",
    ] {
        assert_eq!(LimitKey::new(key).unwrap().as_str(), key);
    }
}

#[test]
fn a_limit_key_outside_its_rule_is_refused() {
    for key in ["", "Five-hour", "5h", "seven day", "a/b", &"a".repeat(65)] {
        assert!(LimitKey::new(key).is_err(), "{key:?} was admitted");
    }
}

#[test]
fn a_used_share_past_the_limit_is_kept() {
    assert_eq!(window("spend", 104.5).used_percent(), 104.5);
}

#[test]
fn a_used_share_that_is_negative_or_not_a_number_is_refused() {
    for used_percent in [-0.5, f64::NAN, f64::INFINITY] {
        let refused = QuotaWindow::new(
            LimitKey::new("five-hour").unwrap(),
            None,
            None,
            used_percent,
            None,
        );
        assert!(refused.is_err(), "{used_percent} was admitted");
    }
}

#[test]
fn a_window_of_zero_seconds_is_refused() {
    let refused = QuotaWindow::new(
        LimitKey::new("five-hour").unwrap(),
        None,
        Some(0),
        1.0,
        None,
    );
    assert!(refused.is_err());
}

#[test]
fn an_empty_or_control_character_scope_is_refused() {
    for scope in ["", "  ", "Son\nnet", &"s".repeat(65)] {
        let refused = QuotaWindow::new(
            LimitKey::new("seven-day:sonnet").unwrap(),
            Some(scope.to_owned()),
            None,
            1.0,
            None,
        );
        assert!(refused.is_err(), "{scope:?} was admitted");
    }
}

#[test]
fn a_reading_keeps_its_plan_windows_and_block() {
    let reading = QuotaReading::new(
        Some("max".to_owned()),
        vec![window("five-hour", 12.0), window("seven-day", 40.0)],
        true,
    )
    .unwrap();

    assert_eq!(reading.plan(), Some("max"));
    assert_eq!(reading.windows().len(), 2);
    assert!(reading.is_blocked());
}

#[test]
fn a_reading_with_one_limit_twice_is_refused() {
    let refused = QuotaReading::new(
        None,
        vec![window("five-hour", 12.0), window("five-hour", 13.0)],
        false,
    );
    assert_eq!(refused.unwrap_err().value, "five-hour");
}

#[test]
fn a_reading_with_too_many_windows_is_refused() {
    let windows = (0..=MAX_WINDOWS)
        .map(|index| window(&format!("limit{index}"), 1.0))
        .collect();
    assert!(QuotaReading::new(None, windows, false).is_err());
}

#[test]
fn an_empty_plan_name_is_refused() {
    assert!(QuotaReading::new(Some(String::new()), Vec::new(), false).is_err());
}
