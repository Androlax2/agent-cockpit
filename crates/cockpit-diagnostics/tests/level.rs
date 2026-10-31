mod support;

use std::thread;
use std::time::Duration;

use cockpit_diagnostics::{Directives, Error, LevelOverride};
use support::{messages, record};

#[test]
fn the_default_level_is_info() {
    assert_eq!(
        Directives::from_env_value(None).unwrap().to_string(),
        "info"
    );
}

#[test]
fn per_component_directives_are_accepted() {
    let directives = Directives::from_env_value(Some("scheduling=debug,tmux=trace")).unwrap();
    assert_eq!(directives.to_string(), "scheduling=debug,tmux=trace");
}

#[test]
fn an_invalid_level_fails_instead_of_falling_back() {
    let error = Directives::from_env_value(Some("loud")).unwrap_err();
    assert!(matches!(error, Error::InvalidDirectives { .. }), "{error}");
}

#[test]
fn the_level_filters_records() {
    let log_dir = tempfile::tempdir().unwrap();
    record(log_dir.path(), "info", |_| {
        tracing::debug!("hidden detail");
        tracing::info!("session started");
    });
    assert_eq!(messages(log_dir.path()), ["session started"]);
}

#[test]
fn an_override_without_duration_lasts_five_minutes() {
    let level_override = LevelOverride::parse("debug", None).unwrap();
    assert_eq!(level_override.duration(), Duration::from_secs(5 * 60));
}

#[test]
fn an_override_duration_is_read_in_minutes_or_seconds() {
    assert_eq!(
        LevelOverride::parse("debug", Some("15m"))
            .unwrap()
            .duration(),
        Duration::from_secs(900)
    );
    assert_eq!(
        LevelOverride::parse("debug", Some("90s"))
            .unwrap()
            .duration(),
        Duration::from_secs(90)
    );
}

#[test]
fn an_override_longer_than_fifteen_minutes_is_rejected() {
    let error = LevelOverride::parse("trace", Some("16m")).unwrap_err();
    assert!(matches!(error, Error::OverrideTooLong { .. }), "{error}");
}

#[test]
fn an_override_with_an_unreadable_duration_is_rejected() {
    for input in ["", "0s", "15", "m", "1h", "-1m"] {
        let error = LevelOverride::parse("debug", Some(input)).unwrap_err();
        assert!(
            matches!(error, Error::InvalidDuration { .. }),
            "{input:?}: {error}"
        );
    }
}

#[test]
fn an_override_raises_the_running_filter() {
    let log_dir = tempfile::tempdir().unwrap();
    record(log_dir.path(), "info", |diagnostics| {
        let level_override = LevelOverride::parse("debug", Some("1m")).unwrap();
        diagnostics.level().set_override(level_override).unwrap();
        tracing::debug!("admission math");
    });
    assert!(messages(log_dir.path()).contains(&"admission math".to_owned()));
}

#[test]
fn an_override_reverts_when_it_expires() {
    let log_dir = tempfile::tempdir().unwrap();
    record(log_dir.path(), "info", |diagnostics| {
        let level_override =
            LevelOverride::new("debug".parse().unwrap(), Duration::from_millis(50)).unwrap();
        diagnostics.level().set_override(level_override).unwrap();
        thread::sleep(Duration::from_millis(500));
        tracing::debug!("after expiry");
        assert_eq!(diagnostics.level().status().active_override, None);
    });
    assert!(!messages(log_dir.path()).contains(&"after expiry".to_owned()));
}

#[test]
fn a_replaced_override_is_not_reverted_by_the_earlier_timer() {
    let log_dir = tempfile::tempdir().unwrap();
    record(log_dir.path(), "info", |diagnostics| {
        let short =
            LevelOverride::new("debug".parse().unwrap(), Duration::from_millis(50)).unwrap();
        diagnostics.level().set_override(short).unwrap();
        let long = LevelOverride::parse("debug", Some("1m")).unwrap();
        diagnostics.level().set_override(long).unwrap();
        thread::sleep(Duration::from_millis(500));
        tracing::debug!("still raised");
    });
    assert!(messages(log_dir.path()).contains(&"still raised".to_owned()));
}

#[test]
fn reverting_restores_the_starting_directives() {
    let log_dir = tempfile::tempdir().unwrap();
    record(log_dir.path(), "info", |diagnostics| {
        let level_override = LevelOverride::parse("debug", None).unwrap();
        diagnostics.level().set_override(level_override).unwrap();
        diagnostics.level().revert().unwrap();
        tracing::debug!("after revert");
        let status = diagnostics.level().status();
        assert_eq!(status.directives.to_string(), "info");
        assert_eq!(status.active_override, None);
    });
    assert!(!messages(log_dir.path()).contains(&"after revert".to_owned()));
}

#[test]
fn level_changes_are_logged() {
    let log_dir = tempfile::tempdir().unwrap();
    record(log_dir.path(), "info", |diagnostics| {
        let level_override = LevelOverride::parse("debug", Some("2m")).unwrap();
        diagnostics.level().set_override(level_override).unwrap();
        diagnostics.level().revert().unwrap();
    });
    let messages = messages(log_dir.path());
    assert!(
        messages.contains(&"log level raised".to_owned()),
        "{messages:?}"
    );
    assert!(
        messages.contains(&"log level reverted".to_owned()),
        "{messages:?}"
    );
}
