//! Alerts as a window fills: once per threshold and window instance, never again after a
//! restart.
mod common;

use cockpit_provider_api::QuotaReading;
use cockpit_usage::QuotaPolicy;
use common::{Fixture, HOUR, NOW, RESET, five_hour, thresholds, window};

fn five_hour_without_reset(used_percent: f64) -> QuotaReading {
    QuotaReading::new(None, vec![window("five-hour", used_percent, None)], false).unwrap()
}

#[tokio::test]
async fn a_window_passing_a_threshold_fires_its_alert_once() {
    let fixture = Fixture::new().await;
    assert!(
        fixture
            .record(&five_hour(70.0, RESET), NOW)
            .await
            .is_empty()
    );

    let passing = fixture.record(&five_hour(81.0, RESET), NOW + 60).await;
    let still_past = fixture.record(&five_hour(85.0, RESET), NOW + 120).await;

    assert_eq!(thresholds(&passing), [("five-hour".to_owned(), 80)]);
    let alert = &passing[0];
    assert_eq!(
        (alert.provider.as_str(), alert.account.as_str()),
        ("claude", "main")
    );
    assert_eq!(
        (alert.used_percent, alert.resets_at, alert.fired_at),
        (81.0, Some(RESET), NOW + 60)
    );
    assert!(still_past.is_empty(), "{still_past:#?}");
}

#[tokio::test]
async fn each_threshold_fires_as_the_window_fills() {
    let fixture = Fixture::new().await;

    let at_80 = fixture.record(&five_hour(80.0, RESET), NOW).await;
    let at_95 = fixture.record(&five_hour(95.0, RESET), NOW + 60).await;

    assert_eq!(thresholds(&at_80), [("five-hour".to_owned(), 80)]);
    assert_eq!(thresholds(&at_95), [("five-hour".to_owned(), 95)]);
}

#[tokio::test]
async fn a_jump_past_several_thresholds_fires_the_highest_only() {
    let fixture = Fixture::new().await;

    let jump = fixture.record(&five_hour(97.0, RESET), NOW).await;
    let later = fixture.record(&five_hour(98.0, RESET), NOW + 60).await;

    assert_eq!(thresholds(&jump), [("five-hour".to_owned(), 95)]);
    assert!(later.is_empty(), "{later:#?}");
}

#[tokio::test]
async fn the_next_window_fires_again() {
    let fixture = Fixture::new().await;
    fixture.record(&five_hour(90.0, RESET), NOW).await;

    let next_window = fixture
        .record(&five_hour(82.0, RESET + 5 * HOUR), RESET + HOUR)
        .await;

    assert_eq!(thresholds(&next_window), [("five-hour".to_owned(), 80)]);
}

#[tokio::test]
async fn a_reset_time_moving_by_a_fraction_is_the_same_window() {
    let fixture = Fixture::new().await;
    fixture.record(&five_hour(82.0, RESET - 1), NOW).await;

    let jittered = fixture.record(&five_hour(83.0, RESET), NOW + 60).await;

    assert!(jittered.is_empty(), "{jittered:#?}");
}

#[tokio::test]
async fn fired_alerts_survive_a_restart() {
    let fixture = Fixture::new().await;
    fixture.record(&five_hour(82.0, RESET), NOW).await;
    let fixture = fixture.reopened().await;

    let after_restart = fixture.record(&five_hour(84.0, RESET), NOW + 60).await;

    assert!(after_restart.is_empty(), "{after_restart:#?}");
}

#[tokio::test]
async fn a_window_without_a_reset_time_re_arms_once_it_drops_below() {
    let fixture = Fixture::new().await;
    fixture.record(&five_hour_without_reset(85.0), NOW).await;
    assert!(
        fixture
            .record(&five_hour_without_reset(86.0), NOW + 60)
            .await
            .is_empty()
    );
    assert!(
        fixture
            .record(&five_hour_without_reset(5.0), NOW + 120)
            .await
            .is_empty()
    );

    let refilled = fixture
        .record(&five_hour_without_reset(81.0), NOW + 180)
        .await;

    assert_eq!(thresholds(&refilled), [("five-hour".to_owned(), 80)]);
}

#[tokio::test]
async fn a_window_already_reset_fires_nothing() {
    let fixture = Fixture::new().await;

    let over = fixture.record(&five_hour(99.0, NOW), NOW).await;

    assert!(over.is_empty(), "{over:#?}");
}

#[tokio::test]
async fn each_window_and_account_fires_on_its_own() {
    let fixture = Fixture::new().await;
    let both = QuotaReading::new(
        None,
        vec![
            window("five-hour", 96.0, Some(RESET)),
            window("seven-day", 81.0, Some(NOW + 50 * HOUR)),
        ],
        false,
    )
    .unwrap();

    let main = fixture.record(&both, NOW).await;
    let second = fixture
        .record_for("claude", "second", &both, NOW)
        .await
        .unwrap();

    let expected = [("five-hour".to_owned(), 95), ("seven-day".to_owned(), 80)];
    assert_eq!(thresholds(&main), expected);
    assert_eq!(thresholds(&second), expected);
    assert_eq!(second[0].account.as_str(), "second");
}

#[tokio::test]
async fn the_owners_thresholds_decide_when_alerts_fire() {
    let mut fixture = Fixture::new().await;
    fixture.policy = QuotaPolicy::new(vec![50], 600).unwrap();

    let at_60 = fixture.record(&five_hour(60.0, RESET), NOW).await;
    let at_99 = fixture.record(&five_hour(99.0, RESET), NOW + 60).await;

    assert_eq!(thresholds(&at_60), [("five-hour".to_owned(), 50)]);
    assert!(at_99.is_empty(), "{at_99:#?}");
}

#[tokio::test]
async fn a_reset_time_moving_across_a_half_minute_is_the_same_window() {
    let fixture = Fixture::new().await;
    fixture.record(&five_hour(96.0, RESET + 29), NOW).await;

    let jittered = fixture.record(&five_hour(97.0, RESET + 31), NOW + 60).await;

    assert!(jittered.is_empty(), "{jittered:#?}");
}

#[tokio::test]
async fn a_window_left_out_of_one_reading_does_not_alert_again_when_it_returns() {
    let fixture = Fixture::new().await;
    let both = |five_hour_used| {
        QuotaReading::new(
            None,
            vec![
                window("five-hour", five_hour_used, Some(RESET)),
                window("seven-day", 96.0, Some(NOW + 50 * HOUR)),
            ],
            false,
        )
        .unwrap()
    };
    fixture.record(&both(10.0), NOW).await;
    fixture.record(&five_hour(11.0, RESET), NOW + 60).await;

    let returned = fixture.record(&both(12.0), NOW + 120).await;

    assert!(returned.is_empty(), "{returned:#?}");
}

#[tokio::test]
async fn a_reading_with_its_reset_already_past_keeps_the_fired_alerts() {
    let fixture = Fixture::new().await;
    fixture.record(&five_hour(96.0, RESET), NOW).await;
    assert!(
        fixture
            .record(&five_hour(96.0, RESET), RESET + 1)
            .await
            .is_empty()
    );

    let corrected = fixture
        .record(&five_hour(96.0, RESET + 30), RESET + 2)
        .await;

    assert!(corrected.is_empty(), "{corrected:#?}");
}

#[tokio::test]
async fn a_threshold_added_below_one_already_fired_stays_silent() {
    let mut fixture = Fixture::new().await;
    fixture.record(&five_hour(96.0, RESET), NOW).await;
    fixture.policy = QuotaPolicy::new(vec![50, 80, 95], 600).unwrap();

    let after_change = fixture.record(&five_hour(97.0, RESET), NOW + 60).await;

    assert!(after_change.is_empty(), "{after_change:#?}");
}
