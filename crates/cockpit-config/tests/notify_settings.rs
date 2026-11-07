//! The owner's `[notify.desktop]` and `[notify.sound]` settings, read through
//! `cockpit_config::load` from an isolated home: which events `torlek notify` tells with a
//! desktop notification and which with a sound (#62, #682).

use std::fs;
use std::path::Path;

use cockpit_config::{ConfigError, EffectiveConfig, Environment, NOTIFY_EVENTS, Source};
use tempfile::TempDir;

fn write_user_config(home: &Path, contents: &str) {
    let config_dir = home.join(".config/torlek");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(config_dir.join("config.toml"), contents).unwrap();
}

fn load(home: &Path, project_dir: Option<&Path>) -> Result<EffectiveConfig, ConfigError> {
    let environment = Environment {
        home: Some(home.into()),
        ..Environment::default()
    };
    cockpit_config::load(&environment, project_dir, &[])
}

#[test]
fn a_clean_home_tells_every_event_but_turn_both_ways() {
    let home = TempDir::new().unwrap();

    let config = load(home.path(), None).unwrap();

    for events in [&config.notify.desktop, &config.notify.sound] {
        for (name, setting) in events.by_name() {
            assert_eq!(setting.value, name != "turn", "{name}");
            assert_eq!(setting.source, Source::ProductDefault, "{name}");
        }
        assert_eq!(events.to_argument(), "failure,hold,question,quota,finished");
    }
}

#[test]
fn the_owner_turns_one_sound_off_with_its_line_and_keeps_its_notification() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[notify.sound]\nfinished = false\n");

    let config = load(home.path(), None).unwrap();

    assert!(!config.notify.sound.finished.value);
    assert_eq!(
        config.notify.sound.finished.source,
        Source::UserFile {
            path: config.user_file.path.clone(),
            line: 2
        }
    );
    assert!(config.notify.desktop.finished.value);
    assert_eq!(
        config.notify.sound.to_argument(),
        "failure,hold,question,quota"
    );
}

#[test]
fn every_event_off_is_none() {
    let home = TempDir::new().unwrap();
    let off: String = NOTIFY_EVENTS
        .iter()
        .map(|event| format!("{event} = false\n"))
        .collect();
    write_user_config(home.path(), &format!("[notify.desktop]\n{off}"));

    let config = load(home.path(), None).unwrap();

    assert_eq!(config.notify.desktop.to_argument(), "none");
}

#[test]
fn an_unknown_event_fails_to_parse() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[notify.sound]\nlogin = false\n");

    assert!(matches!(
        load(home.path(), None),
        Err(ConfigError::Parse { .. })
    ));
}

#[test]
fn a_project_file_cannot_set_the_notify_settings() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();
    fs::create_dir_all(project.path().join(".torlek")).unwrap();
    fs::write(
        project.path().join(".torlek/config.toml"),
        "[notify.desktop]\nfailure = false\n",
    )
    .unwrap();

    let error = load(home.path(), Some(project.path())).unwrap_err();

    assert!(
        matches!(error, ConfigError::OwnerOnly { key: "notify", .. }),
        "{error}"
    );
}

#[test]
fn config_show_prints_each_event_with_its_source() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[notify.desktop]\nquota = false\n");

    let shown = cockpit_config::render(&load(home.path(), None).unwrap());

    assert!(
        shown.contains("notify.sound.question = true  # product default\n"),
        "{shown}"
    );
    assert!(
        shown.contains("notify.desktop.quota = false  # user config"),
        "{shown}"
    );
}

#[test]
fn a_clean_home_has_no_hold_detector() {
    let home = TempDir::new().unwrap();

    assert!(load(home.path(), None).unwrap().notify.holds.is_empty());
}

#[test]
fn the_owner_names_a_hold_detector_by_its_absolute_path() {
    let home = TempDir::new().unwrap();
    write_user_config(
        home.path(),
        "[notify.hold]\nmeeting = \"/usr/local/bin/in-a-meeting\"\n",
    );

    let config = load(home.path(), None).unwrap();

    let meeting = &config.notify.holds["meeting"];
    assert_eq!(meeting.value, Path::new("/usr/local/bin/in-a-meeting"));
    assert_eq!(
        meeting.source,
        Source::UserFile {
            path: config.user_file.path.clone(),
            line: 2
        }
    );
    assert!(
        cockpit_config::render(&config)
            .contains("notify.hold.meeting = \"/usr/local/bin/in-a-meeting\"  # user config"),
        "{}",
        cockpit_config::render(&config)
    );
}

#[test]
fn a_hold_detector_that_cannot_be_run_as_named_is_refused() {
    for (line, reason) in [
        ("meeting = \"in-a-meeting\"", "absolute path"),
        ("away = \"/bin/true\"", "torlek notify away on"),
        ("Meeting = \"/bin/true\"", "lowercase letters"),
    ] {
        let home = TempDir::new().unwrap();
        write_user_config(home.path(), &format!("[notify.hold]\n{line}\n"));

        let error = load(home.path(), None).unwrap_err();

        assert!(
            matches!(&error, ConfigError::Invalid { key, .. } if key.starts_with("notify.hold.")),
            "{line}: {error}"
        );
        assert!(error.to_string().contains(reason), "{line}: {error}");
    }
}

#[test]
fn autostart_is_on_by_default() {
    let home = TempDir::new().unwrap();

    let config = load(home.path(), None).unwrap();

    assert!(config.notify.autostart.value);
    assert_eq!(config.notify.autostart.source, Source::ProductDefault);
}

#[test]
fn the_owner_turns_autostart_off_and_config_show_says_where() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[notify]\nautostart = false\n");

    let config = load(home.path(), None).unwrap();

    assert!(!config.notify.autostart.value);
    assert!(
        cockpit_config::render(&config).contains("notify.autostart = false  # user config"),
        "{}",
        cockpit_config::render(&config)
    );
}

#[test]
fn the_owner_turns_on_the_completed_turn_event_for_the_desktop_only() {
    let home = TempDir::new().unwrap();
    write_user_config(home.path(), "[notify.desktop]\nturn = true\n");

    let config = load(home.path(), None).unwrap();

    assert!(config.notify.desktop.turn.value);
    assert!(!config.notify.sound.turn.value);
    assert_eq!(
        config.notify.desktop.to_argument(),
        format!("{},turn", "failure,hold,question,quota,finished")
    );
}
