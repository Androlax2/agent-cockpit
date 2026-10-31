//! Start-file change detection through the public API sessions will call: declare the files,
//! snapshot at launch, ask what changed.

use std::fs::{self, File};
use std::path::Path;
use std::time::{Duration, SystemTime};

use cockpit_config::{Environment, StartFile, StartFileSnapshot};
use tempfile::TempDir;

fn start_file(dir: &Path, label: &str, providers: &[&str]) -> StartFile {
    StartFile {
        label: label.to_string(),
        path: dir.join(label),
        providers: providers.iter().map(|name| name.to_string()).collect(),
    }
}

/// Writes `contents` and pins the modification time, so tests do not depend on clock ticks.
fn write_at(path: &Path, contents: &str, modified: SystemTime) {
    fs::write(path, contents).unwrap();
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
}

fn at(seconds: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + seconds)
}

#[test]
fn nothing_changed_reports_nothing() {
    let dir = TempDir::new().unwrap();
    let files = [start_file(dir.path(), "settings", &["claude"])];
    write_at(&files[0].path, "{}", at(0));
    let snapshot = StartFileSnapshot::capture(&files, "claude");
    assert_eq!(snapshot.changed(&files), Vec::<&str>::new());
}

#[test]
fn every_kind_of_change_to_a_read_file_is_reported() {
    type Change = fn(&Path);
    let cases: [(&str, Change); 4] = [
        ("edited", |path| write_at(path, "{\"a\":1}", at(10))),
        ("touched", |path| write_at(path, "{}", at(10))),
        ("replaced by an older copy", |path| {
            write_at(path, "{}", at(0))
        }),
        ("deleted", |path| fs::remove_file(path).unwrap()),
    ];
    for (change, apply) in cases {
        let dir = TempDir::new().unwrap();
        let files = [start_file(dir.path(), "settings", &["claude"])];
        write_at(&files[0].path, "{}", at(5));
        let snapshot = StartFileSnapshot::capture(&files, "claude");
        apply(&files[0].path);
        assert_eq!(snapshot.changed(&files), ["settings"], "{change}");
    }
}

#[test]
fn a_same_size_copy_that_keeps_the_old_modification_time_is_reported() {
    let dir = TempDir::new().unwrap();
    let files = [start_file(dir.path(), "settings", &["claude"])];
    write_at(&files[0].path, "{\"a\":1}", at(5));
    let snapshot = StartFileSnapshot::capture(&files, "claude");
    // What `cp -p` or a backup restore does: new content, same length, same mtime.
    std::thread::sleep(Duration::from_millis(20));
    write_at(&files[0].path, "{\"b\":2}", at(5));
    assert_eq!(snapshot.changed(&files), ["settings"]);
}

#[test]
fn a_label_pointed_at_another_path_after_launch_is_reported() {
    let dir = TempDir::new().unwrap();
    let at_launch = [start_file(dir.path(), "launcher", &["claude"])];
    let snapshot = StartFileSnapshot::capture(&at_launch, "claude");
    let repointed = [StartFile {
        path: dir.path().join("other-launcher"),
        ..at_launch[0].clone()
    }];
    assert_eq!(snapshot.changed(&repointed), ["launcher"]);
}

#[test]
fn a_file_created_after_launch_is_reported() {
    let dir = TempDir::new().unwrap();
    let files = [start_file(dir.path(), "CLAUDE.md", &["claude"])];
    let snapshot = StartFileSnapshot::capture(&files, "claude");
    write_at(&files[0].path, "# new", at(0));
    assert_eq!(snapshot.changed(&files), ["CLAUDE.md"]);
}

#[test]
fn a_file_missing_before_and_after_is_not_a_change() {
    let dir = TempDir::new().unwrap();
    let files = [start_file(dir.path(), "CLAUDE.md", &["claude"])];
    let snapshot = StartFileSnapshot::capture(&files, "claude");
    assert_eq!(snapshot.changed(&files), Vec::<&str>::new());
}

#[test]
fn a_change_to_a_file_another_provider_reads_is_not_reported() {
    let dir = TempDir::new().unwrap();
    let files = [
        start_file(dir.path(), "codex hooks", &["codex"]),
        start_file(dir.path(), "settings", &["claude"]),
    ];
    write_at(&files[0].path, "{}", at(0));
    let snapshot = StartFileSnapshot::capture(&files, "claude");
    write_at(&files[0].path, "{\"hooks\":[]}", at(10));
    assert_eq!(snapshot.changed(&files), Vec::<&str>::new());
}

#[test]
fn a_file_declared_after_launch_is_reported_because_the_session_cannot_have_it() {
    let dir = TempDir::new().unwrap();
    let declared_at_launch = [start_file(dir.path(), "settings", &["claude"])];
    let snapshot = StartFileSnapshot::capture(&declared_at_launch, "claude");
    let declared_now = [
        declared_at_launch[0].clone(),
        start_file(dir.path(), "launcher", &["claude"]),
    ];
    assert_eq!(snapshot.changed(&declared_now), ["launcher"]);
}

#[test]
fn the_declared_list_from_configuration_drives_detection() {
    let home = TempDir::new().unwrap();
    let config_dir = home.path().join(".config/cockpit");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.toml"),
        "[[start_files]]\nlabel = \"launcher\"\npath = \"~/launcher\"\nproviders = [\"claude\"]\n",
    )
    .unwrap();
    let environment = Environment {
        home: Some(home.path().into()),
        ..Environment::default()
    };
    let start_files = cockpit_config::load(&environment, None, &[])
        .unwrap()
        .declared_start_files();

    let snapshot = StartFileSnapshot::capture(&start_files, "claude");
    write_at(&home.path().join("launcher"), "#!/bin/sh", at(0));
    assert_eq!(snapshot.changed(&start_files), ["launcher"]);
}
