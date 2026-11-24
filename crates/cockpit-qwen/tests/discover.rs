//! Discovery of the installed `qwen`, with fake programs.

mod common;

use std::ffi::OsString;
use std::time::Duration;

use cockpit_provider_api::Provider;
use cockpit_qwen::QwenCode;

#[test]
fn the_version_qwen_prints_is_the_adapter_s() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(dir.path(), "qwen", "echo 0.28.4");

    let qwen = QwenCode::discover(&program, None).unwrap();

    assert_eq!(qwen.version(), Some("0.28.4"));
}

#[test]
fn a_program_that_prints_no_version_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(dir.path(), "qwen", "echo 'Qwen Code'");

    let error = QwenCode::discover(&program, None).unwrap_err();

    assert!(error.reason.contains("not a version"), "{error}");
}

#[test]
fn a_failing_program_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(dir.path(), "qwen", "echo broken >&2; exit 1");

    let error = QwenCode::discover(&program, None).unwrap_err();

    assert!(error.reason.contains("broken"), "{error}");
}

#[test]
fn a_program_that_hangs_is_given_up_on() {
    let dir = tempfile::tempdir().unwrap();
    let program = common::script(dir.path(), "qwen", "sleep 5");

    let error = QwenCode::discover_within(&program, None, Duration::from_millis(200)).unwrap_err();

    assert!(error.reason.contains("did not answer"), "{error}");
}

#[test]
fn the_first_executable_qwen_on_path_is_found() {
    let empty = tempfile::tempdir().unwrap();
    let not_executable = tempfile::tempdir().unwrap();
    std::fs::write(not_executable.path().join("qwen"), "").unwrap();
    let installed = tempfile::tempdir().unwrap();
    let program = common::script(installed.path(), "qwen", "echo 0.28.4");
    let path =
        std::env::join_paths([empty.path(), not_executable.path(), installed.path()]).unwrap();

    assert_eq!(QwenCode::find_on_path(&path), Some(program));
    assert_eq!(QwenCode::find_on_path(&OsString::from(empty.path())), None);
}
