//! Per-project answers directories on a real file system: private, write-once, read-only.

use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use cockpit_agent_channel::{AnswersDirectory, AnswersError, ProjectKey};

/// A private runtime directory, as the daemon keeps it, and the answers root inside it.
fn answers_root() -> (tempfile::TempDir, PathBuf) {
    let runtime = tempfile::tempdir().unwrap();
    fs::set_permissions(runtime.path(), fs::Permissions::from_mode(0o700)).unwrap();
    let root = runtime.path().join("answers");
    (runtime, root)
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn project(key: &str) -> ProjectKey {
    ProjectKey::new(key).unwrap()
}

#[test]
fn preparing_creates_the_root_and_the_project_directory_private() {
    let (_runtime, root) = answers_root();

    let answers = AnswersDirectory::prepare(&root, &project("agent-cockpit")).unwrap();

    assert_eq!(answers.path(), root.join("agent-cockpit"));
    assert_eq!(mode(&root), 0o700);
    assert_eq!(mode(answers.path()), 0o700);
}

#[test]
fn an_answer_is_written_whole_and_read_only() {
    let (_runtime, root) = answers_root();
    let answers = AnswersDirectory::prepare(&root, &project("agent-cockpit")).unwrap();

    let written = answers.write("run-1.json", b"{\"exit\":0}").unwrap();

    assert_eq!(fs::read(&written).unwrap(), b"{\"exit\":0}");
    assert_eq!(mode(&written), 0o400);
    let leftovers: Vec<_> = fs::read_dir(answers.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, vec!["run-1.json"]);
}

#[test]
fn a_second_answer_with_the_same_name_is_refused() {
    let (_runtime, root) = answers_root();
    let answers = AnswersDirectory::prepare(&root, &project("agent-cockpit")).unwrap();
    answers.write("run-1.json", b"first").unwrap();

    let second = answers.write("run-1.json", b"second");

    assert!(
        matches!(second, Err(AnswersError::AlreadyAnswered { .. })),
        "{second:?}"
    );
    assert_eq!(
        fs::read(answers.path().join("run-1.json")).unwrap(),
        b"first"
    );
}

#[test]
fn answer_names_that_could_leave_the_directory_are_refused() {
    let (_runtime, root) = answers_root();
    let answers = AnswersDirectory::prepare(&root, &project("agent-cockpit")).unwrap();
    for name in ["", "../escape", "a/b", ".hidden", "..", "tab\tname", "é"] {
        let refused = answers.write(name, b"x");

        assert!(
            matches!(refused, Err(AnswersError::InvalidName { .. })),
            "{name:?}: {refused:?}"
        );
    }
}

#[test]
fn project_keys_outside_letters_digits_and_dashes_are_refused() {
    for key in ["", ".", "..", "a/b", "a_b", "two words", &"x".repeat(256)] {
        assert!(ProjectKey::new(key).is_err(), "{key:?}");
    }
}

#[test]
fn a_symbolic_link_in_place_of_the_project_directory_is_refused() {
    let (runtime, root) = answers_root();
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let elsewhere = runtime.path().join("elsewhere");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&elsewhere)
        .unwrap();
    std::os::unix::fs::symlink(&elsewhere, root.join("agent-cockpit")).unwrap();

    let refused = AnswersDirectory::prepare(&root, &project("agent-cockpit"));

    assert!(
        matches!(refused, Err(AnswersError::Unsafe { .. })),
        "{refused:?}"
    );
}

#[test]
fn a_project_directory_others_may_open_is_refused_not_tightened() {
    let (_runtime, root) = answers_root();
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let project_dir = root.join("agent-cockpit");
    fs::create_dir(&project_dir).unwrap();
    fs::set_permissions(&project_dir, fs::Permissions::from_mode(0o755)).unwrap();

    let refused = AnswersDirectory::prepare(&root, &project("agent-cockpit"));

    assert!(
        matches!(refused, Err(AnswersError::Unsafe { .. })),
        "{refused:?}"
    );
    assert_eq!(mode(&project_dir), 0o755);
}
