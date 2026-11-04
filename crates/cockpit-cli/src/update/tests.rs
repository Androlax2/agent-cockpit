//! Version and channel comparison, release-list reading and the notice schedule, on fixture
//! release data and an explicit clock: no network, no disk but a temporary directory.

use serde_json::json;

use super::release::{Channel, Installed, Latest, Version, latest_release};
use super::state::{CHECK_INTERVAL_SECONDS, POSTPONE_SECONDS, UpdateState, load, save};

const PLATFORM: &str = "linux-x86_64";
const DAY: i64 = CHECK_INTERVAL_SECONDS;

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn installed(text: &str) -> Installed {
    Installed::from_manifest(&json!({"version": text, "platform": PLATFORM}).to_string()).unwrap()
}

/// A release as GitHub lists it, with the archive and checksums of `platforms`.
fn release(tag: &str, platforms: &[&str]) -> serde_json::Value {
    let mut assets: Vec<_> = platforms
        .iter()
        .map(|platform| json!({"name": format!("torlek-{tag}-{platform}.tar.gz")}))
        .collect();
    assets.push(json!({"name": "SHA256SUMS"}));
    json!({"tag_name": tag, "draft": false, "prerelease": true, "body": "notes", "assets": assets})
}

fn list(releases: Vec<serde_json::Value>) -> String {
    serde_json::Value::Array(releases).to_string()
}

fn offered(releases: Vec<serde_json::Value>, installed_version: &str) -> Option<String> {
    match latest_release(&list(releases), &installed(installed_version)).unwrap() {
        Latest::Available(offer) => Some(offer.version.to_string()),
        Latest::UpToDate | Latest::NoArchiveForPlatform(_) => None,
    }
}

#[test]
fn versions_order_by_semantic_version_precedence() {
    let ascending = [
        "0.1.0-alpha.2",
        "0.1.0-alpha.3",
        "0.1.0-alpha.10",
        "0.1.0-beta.1",
        "0.1.0",
        "0.1.1",
        "0.2.0",
        "1.0.0",
    ];
    for pair in ascending.windows(2) {
        assert!(
            version(pair[0]) < version(pair[1]),
            "{} should precede {}",
            pair[0],
            pair[1]
        );
    }
}

#[test]
fn versions_that_are_not_releases_are_refused() {
    for text in [
        "",
        "1",
        "1.2",
        "1.2.3.4",
        "01.2.3",
        "1.2.3-",
        "1.2.3-a..b",
        "v1.2.3",
        "1.2.3+build",
        "1.2.x",
    ] {
        assert!(Version::parse(text).is_err(), "{text:?} was accepted");
    }
}

#[test]
fn versions_read_back_as_written() {
    for text in ["0.1.0-alpha.3", "1.2.3", "0.1.0-beta.1"] {
        assert_eq!(version(text).to_string(), text);
    }
}

#[test]
fn only_alpha_beta_and_stable_shapes_have_a_channel() {
    assert_eq!(version("0.1.0-alpha.1").channel(), Some(Channel::Alpha));
    assert_eq!(version("0.1.0-beta.4").channel(), Some(Channel::Beta));
    assert_eq!(version("0.1.0").channel(), Some(Channel::Stable));
    for text in [
        "0.1.0-dev.3",
        "0.1.0-rc.1",
        "0.1.0-alpha",
        "0.1.0-alpha.1.2",
    ] {
        assert_eq!(version(text).channel(), None, "{text}");
    }
}

#[test]
fn an_older_alpha_installation_is_offered_the_newest_alpha() {
    let releases = vec![
        release("v0.1.0-alpha.2", &[PLATFORM]),
        release("v0.1.0-alpha.5", &[PLATFORM]),
        release("v0.1.0-alpha.4", &[PLATFORM]),
    ];
    assert_eq!(
        offered(releases, "0.1.0-alpha.3"),
        Some("0.1.0-alpha.5".to_owned())
    );
}

#[test]
fn an_alpha_installation_is_offered_the_beta_channel_that_follows_it() {
    let releases = vec![
        release("v0.1.0-alpha.4", &[PLATFORM]),
        release("v0.1.0-beta.1", &[PLATFORM]),
    ];
    assert_eq!(
        offered(releases, "0.1.0-alpha.3"),
        Some("0.1.0-beta.1".to_owned())
    );
}

#[test]
fn a_beta_installation_is_never_offered_an_alpha() {
    let releases = vec![release("v0.2.0-alpha.1", &[PLATFORM])];
    assert_eq!(offered(releases, "0.1.0-beta.2"), None);
}

#[test]
fn a_beta_installation_is_offered_a_newer_beta() {
    let releases = vec![
        release("v0.1.0-beta.3", &[PLATFORM]),
        release("v0.1.0-alpha.9", &[PLATFORM]),
    ];
    assert_eq!(
        offered(releases, "0.1.0-beta.2"),
        Some("0.1.0-beta.3".to_owned())
    );
}

#[test]
fn the_same_or_an_older_release_is_not_a_downgrade_offer() {
    let releases = vec![
        release("v0.1.0-alpha.3", &[PLATFORM]),
        release("v0.1.0-alpha.1", &[PLATFORM]),
    ];
    assert_eq!(offered(releases, "0.1.0-alpha.3"), None);
}

#[test]
fn a_draft_release_is_not_offered() {
    let mut draft = release("v0.1.0-alpha.4", &[PLATFORM]);
    draft["draft"] = json!(true);
    assert_eq!(offered(vec![draft], "0.1.0-alpha.3"), None);
}

#[test]
fn tags_in_no_release_channel_are_not_offered() {
    let releases = vec![
        release("v0.1.0-dev.9", &[PLATFORM]),
        release("v0.1.0-rc.1", &[PLATFORM]),
    ];
    assert_eq!(offered(releases, "0.1.0-alpha.3"), None);
}

#[test]
fn a_release_without_this_platforms_archive_is_not_offered_and_is_named() {
    let releases = vec![release("v0.1.0-alpha.4", &["linux-aarch64"])];
    let outcome = latest_release(&list(releases), &installed("0.1.0-alpha.3")).unwrap();
    assert_eq!(
        outcome,
        Latest::NoArchiveForPlatform(version("0.1.0-alpha.4"))
    );
}

#[test]
fn a_release_without_checksums_is_not_offered() {
    let mut without_checksums = release("v0.1.0-alpha.4", &[PLATFORM]);
    without_checksums["assets"] =
        json!([{"name": format!("torlek-v0.1.0-alpha.4-{PLATFORM}.tar.gz")}]);
    assert_eq!(offered(vec![without_checksums], "0.1.0-alpha.3"), None);
}

#[test]
fn the_newest_release_with_this_platforms_archive_is_offered_when_a_newer_one_lacks_it() {
    let releases = vec![
        release("v0.1.0-alpha.5", &["linux-aarch64"]),
        release("v0.1.0-alpha.4", &[PLATFORM]),
    ];
    assert_eq!(
        offered(releases, "0.1.0-alpha.3"),
        Some("0.1.0-alpha.4".to_owned())
    );
}

#[test]
fn a_release_that_does_not_read_as_one_is_skipped() {
    let releases = vec![
        json!({"tag_name": "v0.1.0-alpha.9"}),
        json!({"tag_name": "nightly", "draft": false, "assets": []}),
        release("v0.1.0-alpha.4", &[PLATFORM]),
    ];
    assert_eq!(
        offered(releases, "0.1.0-alpha.3"),
        Some("0.1.0-alpha.4".to_owned())
    );
}

#[test]
fn a_list_in_which_nothing_reads_is_an_error() {
    let outcome = latest_release(r#"[{"tag_name": 5}, {}]"#, &installed("0.1.0-alpha.3"));
    assert!(outcome.is_err(), "{outcome:?}");
}

#[test]
fn malformed_release_metadata_is_an_error() {
    for text in [
        "",
        "not json",
        "{\"message\": \"API rate limit exceeded\"}",
        "null",
        "\"[]\"",
    ] {
        assert!(
            latest_release(text, &installed("0.1.0-alpha.3")).is_err(),
            "{text:?} was accepted"
        );
    }
}

#[test]
fn an_empty_release_list_is_up_to_date() {
    assert_eq!(
        latest_release("[]", &installed("0.1.0-alpha.3")).unwrap(),
        Latest::UpToDate
    );
}

#[test]
fn release_notes_lose_control_characters_and_are_bounded() {
    let mut noisy = release("v0.1.0-alpha.4", &[PLATFORM]);
    noisy["body"] = json!(format!(
        "line one\n\u{1b}[31mred\u{7}\n{}",
        "x".repeat(9000)
    ));
    let Latest::Available(offer) =
        latest_release(&list(vec![noisy]), &installed("0.1.0-alpha.3")).unwrap()
    else {
        panic!("not offered");
    };
    assert!(!offer.notes.contains('\u{1b}') && !offer.notes.contains('\u{7}'));
    assert!(offer.notes.starts_with("line one\n[31mred\n"));
    assert!(offer.notes.chars().count() <= 4000);
}

#[test]
fn release_notes_lose_invisible_and_bidirectional_formatting_characters() {
    let mut reordered = release("v0.1.0-alpha.4", &[PLATFORM]);
    reordered["body"] = json!("safe\u{202E}txet\u{2066}x\u{200B}y\u{FEFF}");
    let Latest::Available(offer) =
        latest_release(&list(vec![reordered]), &installed("0.1.0-alpha.3")).unwrap()
    else {
        panic!("not offered");
    };
    assert_eq!(offer.notes, "safetxetxy");
}

#[test]
fn an_installation_without_a_release_channel_cannot_check() {
    for text in ["0.1.0-dev.3", "0.1.0-rc.1"] {
        let manifest = json!({"version": text, "platform": PLATFORM}).to_string();
        assert!(Installed::from_manifest(&manifest).is_err(), "{text}");
    }
}

#[test]
fn a_manifest_platform_that_is_not_a_file_name_word_is_refused() {
    for platform in ["", "linux x86", "linux;rm", "../x"] {
        let manifest = json!({"version": "0.1.0-alpha.3", "platform": platform}).to_string();
        assert!(Installed::from_manifest(&manifest).is_err(), "{platform:?}");
    }
}

fn state_with_offer(offered_version: &str) -> UpdateState {
    let mut state = UpdateState::default();
    let installed = installed("0.1.0-alpha.3");
    let list = list(vec![release(&format!("v{offered_version}"), &[PLATFORM])]);
    if let Latest::Available(offer) = latest_release(&list, &installed).unwrap() {
        state.latest = Some(offer);
    }
    state
}

#[test]
fn a_first_start_is_due_for_a_check() {
    assert!(UpdateState::default().is_check_due(1_000));
}

#[test]
fn a_check_is_due_only_a_day_after_the_last_attempt() {
    let state = UpdateState {
        last_attempt_at: Some(1_000),
        ..UpdateState::default()
    };
    assert!(!state.is_check_due(1_000 + DAY - 1));
    assert!(state.is_check_due(1_000 + DAY));
}

#[test]
fn a_last_attempt_in_the_future_does_not_silence_checks() {
    let state = UpdateState {
        last_attempt_at: Some(5_000),
        ..UpdateState::default()
    };
    assert!(state.is_check_due(1_000));
}

#[test]
fn disabled_checks_are_never_due_and_never_announce() {
    let mut state = state_with_offer("0.1.0-alpha.4");
    state.automatic_checks = false;
    assert!(!state.is_check_due(1_000));
    assert_eq!(
        state.offer_to_announce(&installed("0.1.0-alpha.3"), 1_000),
        None
    );
}

#[test]
fn a_newer_cached_release_is_announced() {
    let state = state_with_offer("0.1.0-alpha.4");
    let offer = state
        .offer_to_announce(&installed("0.1.0-alpha.3"), 1_000)
        .unwrap();
    assert_eq!(offer.version.to_string(), "0.1.0-alpha.4");
}

#[test]
fn a_cached_release_that_is_no_longer_newer_is_not_announced() {
    let state = state_with_offer("0.1.0-alpha.4");
    assert_eq!(
        state.offer_to_announce(&installed("0.1.0-alpha.4"), 1_000),
        None
    );
}

#[test]
fn an_announcement_is_not_repeated_within_a_day() {
    let mut state = state_with_offer("0.1.0-alpha.4");
    state.last_notified_at = Some(1_000);
    let installed = installed("0.1.0-alpha.3");
    assert_eq!(state.offer_to_announce(&installed, 1_000 + DAY - 1), None);
    assert!(state.offer_to_announce(&installed, 1_000 + DAY).is_some());
}

#[test]
fn later_postpones_the_announcement_for_three_days() {
    let mut state = state_with_offer("0.1.0-alpha.4");
    state.postpone(1_000);
    let installed = installed("0.1.0-alpha.3");
    assert_eq!(
        state.offer_to_announce(&installed, 1_000 + POSTPONE_SECONDS - 1),
        None
    );
    assert!(
        state
            .offer_to_announce(&installed, 1_000 + POSTPONE_SECONDS)
            .is_some()
    );
}

#[test]
fn skip_silences_that_version_but_not_a_newer_one() {
    let mut state = state_with_offer("0.1.0-alpha.4");
    state.skip(version("0.1.0-alpha.4"));
    let installed = installed("0.1.0-alpha.3");
    assert_eq!(state.offer_to_announce(&installed, 1_000), None);
    let newer = state_with_offer("0.1.0-alpha.5");
    let state_with_newer = UpdateState {
        skipped_version: state.skipped_version.clone(),
        ..newer
    };
    assert!(
        state_with_newer
            .offer_to_announce(&installed, 1_000)
            .is_some()
    );
}

#[test]
fn choices_and_the_cache_survive_a_save_and_load() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state/update.json");
    let mut state = state_with_offer("0.1.0-alpha.4");
    state.automatic_checks = false;
    state.last_attempt_at = Some(11);
    state.last_notified_at = Some(12);
    state.postponed_until = Some(13);
    state.skipped_version = Some(version("0.1.0-alpha.2"));
    save(&path, &state).unwrap();
    assert_eq!(load(&path).unwrap(), state);
}

#[test]
fn a_missing_state_file_loads_the_defaults() {
    let directory = tempfile::tempdir().unwrap();
    assert_eq!(
        load(&directory.path().join("update.json")).unwrap(),
        UpdateState::default()
    );
}

#[test]
fn a_damaged_state_file_is_an_error_that_names_it() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("update.json");
    for contents in [
        "{",
        "[]",
        r#"{"automatic_checks": "yes"}"#,
        r#"{"last_attempt_at": "x"}"#,
        r#"{"skipped_version": "banana"}"#,
    ] {
        std::fs::write(&path, contents).unwrap();
        let problem = load(&path).unwrap_err();
        assert!(problem.contains("update.json"), "{contents}: {problem}");
    }
}
