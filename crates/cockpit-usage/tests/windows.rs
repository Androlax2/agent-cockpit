//! An account's plan windows as recorded readings leave them, and what each means now.
mod common;

use cockpit_provider_api::{LimitKey, QuotaReading, QuotaWindow};
use cockpit_usage::{AccountName, Error, QuotaPolicy, WindowState};
use common::{Fixture, HOUR, NOW, RESET, five_hour, window};

fn states(quota: &cockpit_usage::AccountQuota) -> Vec<(&str, WindowState)> {
    quota
        .windows
        .iter()
        .map(|window| (window.limit.as_str(), window.state))
        .collect()
}

#[tokio::test]
async fn a_reading_lists_its_plan_and_windows_by_limit() {
    let fixture = Fixture::new().await;
    let scoped = QuotaWindow::new(
        LimitKey::new("seven-day:sonnet").unwrap(),
        Some("Sonnet".to_owned()),
        Some(604_800),
        37.4,
        Some(NOW + 50 * HOUR),
    )
    .unwrap();
    let reading = QuotaReading::new(
        Some("max".to_owned()),
        vec![
            window("seven-day", 20.0, Some(NOW + 50 * HOUR)),
            scoped,
            window("five-hour", 5.0, Some(RESET)),
        ],
        false,
    )
    .unwrap();
    fixture.record(&reading, NOW).await;

    let quota = fixture.only_account_at(NOW).await;

    assert_eq!(
        (
            quota.provider.as_str(),
            quota.account.as_str(),
            quota.plan.as_deref()
        ),
        ("claude", "main", Some("max"))
    );
    assert_eq!(quota.observed_at, Some(NOW));
    assert!(quota.is_current);
    let limits: Vec<_> = quota
        .windows
        .iter()
        .map(|window| window.limit.as_str())
        .collect();
    assert_eq!(limits, ["five-hour", "seven-day", "seven-day:sonnet"]);
    let sonnet = &quota.windows[2];
    assert_eq!(sonnet.scope.as_deref(), Some("Sonnet"));
    assert_eq!(sonnet.length_seconds, Some(604_800));
    assert_eq!(sonnet.used_percent, 37.4);
    assert_eq!(sonnet.resets_at, Some(NOW + 50 * HOUR));
}

#[tokio::test]
async fn a_window_is_judged_by_how_much_of_it_is_used() {
    let fixture = Fixture::new().await;
    let reading = QuotaReading::new(
        None,
        vec![
            window("a-available", 79.9, Some(RESET)),
            window("b-low", 80.0, Some(RESET)),
            window("c-exhausted", 100.0, Some(RESET)),
            window("d-past-limit", 104.0, Some(RESET)),
        ],
        false,
    )
    .unwrap();
    fixture.record(&reading, NOW).await;

    let quota = fixture.only_account_at(NOW).await;

    assert_eq!(
        states(&quota),
        [
            ("a-available", WindowState::Available),
            ("b-low", WindowState::Low),
            ("c-exhausted", WindowState::Exhausted),
            ("d-past-limit", WindowState::Exhausted),
        ]
    );
}

#[tokio::test]
async fn a_blocked_account_has_no_room_whatever_its_windows_say() {
    let fixture = Fixture::new().await;
    let reading =
        QuotaReading::new(None, vec![window("five-hour", 3.0, Some(RESET))], true).unwrap();
    fixture.record(&reading, NOW).await;

    let quota = fixture.only_account_at(NOW).await;

    assert!(quota.is_blocked, "{quota:#?}");
    assert_eq!(states(&quota), [("five-hour", WindowState::Blocked)]);
}

#[tokio::test]
async fn a_reading_older_than_the_policy_allows_is_stale() {
    let fixture = Fixture::new().await;
    fixture.record(&five_hour(10.0, RESET), NOW).await;
    let fresh_for = i64::from(fixture.policy.fresh_for_seconds());

    let still_current = fixture.only_account_at(NOW + fresh_for).await;
    let too_old = fixture.only_account_at(NOW + fresh_for + 1).await;

    assert_eq!(
        states(&still_current),
        [("five-hour", WindowState::Available)]
    );
    assert!(!too_old.is_current);
    assert_eq!(states(&too_old), [("five-hour", WindowState::Stale)]);
}

#[tokio::test]
async fn a_reading_from_the_future_is_not_current() {
    let fixture = Fixture::new().await;
    fixture.record(&five_hour(10.0, RESET), NOW + 60).await;

    let quota = fixture.only_account_at(NOW).await;

    assert_eq!(states(&quota), [("five-hour", WindowState::Stale)]);
}

#[tokio::test]
async fn a_window_whose_reset_is_past_is_stale_not_available() {
    let fixture = Fixture::new().await;
    fixture.record(&five_hour(100.0, NOW + 60), NOW).await;

    let quota = fixture.only_account_at(NOW + 60).await;

    assert_eq!(states(&quota), [("five-hour", WindowState::Stale)]);
}

#[tokio::test]
async fn a_failed_refresh_keeps_the_last_reading_and_makes_it_stale() {
    let fixture = Fixture::new().await;
    fixture.record(&five_hour(10.0, RESET), NOW).await;
    fixture
        .fail("usage request timed out", NOW + 60)
        .await
        .unwrap();

    let quota = fixture.only_account_at(NOW + 60).await;

    assert!(!quota.is_current);
    assert_eq!(quota.windows[0].used_percent, 10.0);
    assert_eq!(states(&quota), [("five-hour", WindowState::Stale)]);
    let failure = quota.failure.unwrap();
    assert_eq!(
        (failure.at, failure.reason.as_str()),
        (NOW + 60, "usage request timed out")
    );
}

#[tokio::test]
async fn a_reading_after_a_failure_is_current_again() {
    let fixture = Fixture::new().await;
    fixture.fail("usage request timed out", NOW).await.unwrap();
    fixture.record(&five_hour(10.0, RESET), NOW + 60).await;

    let quota = fixture.only_account_at(NOW + 60).await;

    assert!(quota.is_current);
    assert_eq!(states(&quota), [("five-hour", WindowState::Available)]);
}

#[tokio::test]
async fn a_reading_in_the_same_second_as_a_failure_is_current() {
    let fixture = Fixture::new().await;
    fixture.fail("usage request timed out", NOW).await.unwrap();
    fixture.record(&five_hour(10.0, RESET), NOW).await;

    let quota = fixture.only_account_at(NOW).await;

    assert!(quota.is_current);
}

#[tokio::test]
async fn an_account_whose_every_refresh_failed_has_no_windows() {
    let fixture = Fixture::new().await;
    fixture.fail("not signed in", NOW).await.unwrap();

    let quota = fixture.only_account_at(NOW).await;

    assert_eq!(quota.observed_at, None);
    assert!(!quota.is_current);
    assert!(quota.windows.is_empty());
    assert_eq!(quota.failure.unwrap().reason, "not signed in");
}

#[tokio::test]
async fn an_older_failure_does_not_replace_a_newer_one() {
    let fixture = Fixture::new().await;
    fixture.fail("newer", NOW).await.unwrap();
    fixture.fail("older", NOW - 60).await.unwrap();

    let quota = fixture.only_account_at(NOW).await;

    assert_eq!(quota.failure.unwrap().reason, "newer");
}

#[tokio::test]
async fn a_failure_reason_is_cut_and_an_empty_one_refused() {
    let fixture = Fixture::new().await;
    assert!(matches!(
        fixture.fail("  ", NOW).await,
        Err(Error::Invalid {
            field: "failure",
            ..
        })
    ));
    fixture.fail(&"x".repeat(2000), NOW).await.unwrap();

    let quota = fixture.only_account_at(NOW).await;

    assert_eq!(
        quota.failure.unwrap().reason.chars().count(),
        cockpit_usage::MAX_FAILURE_CHARS
    );
}

#[tokio::test]
async fn a_new_reading_replaces_every_window_of_the_last() {
    let fixture = Fixture::new().await;
    let both = QuotaReading::new(
        None,
        vec![
            window("five-hour", 10.0, Some(RESET)),
            window("seven-day", 30.0, None),
        ],
        false,
    )
    .unwrap();
    fixture.record(&both, NOW).await;
    fixture.record(&five_hour(12.0, RESET), NOW + 60).await;

    let quota = fixture.only_account_at(NOW + 60).await;

    assert_eq!(states(&quota), [("five-hour", WindowState::Available)]);
    assert_eq!(quota.windows[0].used_percent, 12.0);
}

#[tokio::test]
async fn a_reading_older_than_the_recorded_one_is_refused() {
    let fixture = Fixture::new().await;
    fixture.record(&five_hour(30.0, RESET), NOW).await;

    let late = fixture
        .record_for("claude", "main", &five_hour(10.0, RESET), NOW - 1)
        .await;

    assert!(matches!(
        late,
        Err(Error::OlderReading { observed_at, recorded_at }) if observed_at == NOW - 1 && recorded_at == NOW
    ));
    assert_eq!(
        fixture.only_account_at(NOW).await.windows[0].used_percent,
        30.0
    );
}

#[tokio::test]
async fn two_accounts_on_one_provider_keep_their_own_windows() {
    let fixture = Fixture::new().await;
    fixture
        .record_for("claude", "main", &five_hour(90.0, RESET), NOW)
        .await
        .unwrap();
    fixture
        .record_for("claude", "second", &five_hour(5.0, RESET), NOW)
        .await
        .unwrap();
    fixture
        .record_for("codex", "main", &five_hour(50.0, RESET), NOW)
        .await
        .unwrap();

    let quotas = fixture.quotas_at(NOW).await;

    let used: Vec<_> = quotas
        .iter()
        .map(|quota| {
            (
                quota.provider.as_str(),
                quota.account.as_str(),
                quota.windows[0].used_percent,
            )
        })
        .collect();
    assert_eq!(
        used,
        [
            ("claude", "main", 90.0),
            ("claude", "second", 5.0),
            ("codex", "main", 50.0)
        ]
    );
}

#[test]
fn an_account_name_is_lower_case_letters_digits_and_dashes() {
    for name in ["main", "work-2"] {
        assert_eq!(AccountName::new(name).unwrap().as_str(), name);
    }
    for name in ["", "Main", "2nd", "a b", "a@b.com", &"a".repeat(33)] {
        assert!(AccountName::new(name).is_err(), "{name:?} was admitted");
    }
}

#[test]
fn a_policy_takes_ascending_thresholds_and_a_freshness() {
    let policy = QuotaPolicy::new(vec![50, 80, 95], 300).unwrap();
    assert_eq!(policy.alert_thresholds(), [50, 80, 95]);
    assert_eq!(policy.fresh_for_seconds(), 300);
    assert_eq!(QuotaPolicy::default().alert_thresholds(), [80, 95]);
}

#[test]
fn a_policy_outside_its_rules_is_refused() {
    for (thresholds, fresh_for) in [
        (vec![], 300),
        (vec![95, 80], 300),
        (vec![80, 80], 300),
        (vec![0], 300),
        (vec![101], 300),
        (vec![10, 20, 30, 40, 50, 60], 300),
        (vec![80], 0),
    ] {
        assert!(
            QuotaPolicy::new(thresholds.clone(), fresh_for).is_err(),
            "{thresholds:?}, {fresh_for} was admitted"
        );
    }
}
