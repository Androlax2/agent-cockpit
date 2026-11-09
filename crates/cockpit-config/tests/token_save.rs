//! Saving and removing a secret file (#1251): the user guide's modes, and never through a
//! symbolic link.
use std::os::unix::fs::PermissionsExt;

use cockpit_config::{
    TokenWriteError, remove_secret_file, save_github_token, save_provider_credential,
};

const TOKEN: &str = "github_pat_fixture_0123";

fn mode(path: &std::path::Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o7777
}

#[test]
fn a_saved_token_is_alone_on_one_line_with_mode_0600_in_a_new_0700_folder() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("github-tokens/app");

    save_github_token(&path, TOKEN).unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{TOKEN}\n")
    );
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&root.path().join("github-tokens")), 0o700);
    assert_eq!(
        std::fs::read_dir(root.path().join("github-tokens"))
            .unwrap()
            .count(),
        1,
        "no staged file is left behind"
    );
}

#[test]
fn a_symbolic_link_where_the_file_goes_is_refused_and_its_target_untouched() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("elsewhere");
    std::fs::write(&target, "kept\n").unwrap();
    std::fs::create_dir(root.path().join("github-tokens")).unwrap();
    let path = root.path().join("github-tokens/app");
    std::os::unix::fs::symlink(&target, &path).unwrap();

    let error = save_github_token(&path, TOKEN).unwrap_err();

    assert!(matches!(error, TokenWriteError::NotPlain { .. }), "{error}");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "kept\n");
}

#[test]
fn a_symbolic_link_as_the_folder_is_refused() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("real")).unwrap();
    std::os::unix::fs::symlink(root.path().join("real"), root.path().join("claude")).unwrap();

    let error = save_provider_credential(&root.path().join("claude"), "ANTHROPIC_API_KEY", "sk-1")
        .unwrap_err();

    assert!(matches!(error, TokenWriteError::NotPlain { .. }), "{error}");
    assert_eq!(
        std::fs::read_dir(root.path().join("real")).unwrap().count(),
        0
    );
}

#[test]
fn a_refused_value_is_not_in_the_error() {
    let root = tempfile::tempdir().unwrap();

    let error = save_github_token(&root.path().join("app"), "secret with spaces").unwrap_err();

    assert!(!error.to_string().contains("secret"), "{error}");
    assert!(!root.path().join("app").exists());
}

#[test]
fn removing_a_symbolic_link_removes_the_link_not_its_target() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("elsewhere");
    std::fs::write(&target, "kept\n").unwrap();
    let path = root.path().join("app");
    std::os::unix::fs::symlink(&target, &path).unwrap();

    assert!(remove_secret_file(&path).unwrap());

    assert!(std::fs::symlink_metadata(&path).is_err());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "kept\n");
}

#[test]
fn the_longest_token_saved_is_one_a_launch_still_reads() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("app");
    let longest = "a".repeat(1023);

    save_github_token(&path, &longest).unwrap();
    let too_long = save_github_token(&path, &"a".repeat(1024)).unwrap_err();

    let read = cockpit_config::read_github_token(&path).unwrap().unwrap();
    assert_eq!(read.expose(), longest);
    assert!(
        matches!(too_long, TokenWriteError::NotOne { .. }),
        "{too_long}"
    );
}

#[test]
fn saves_of_one_file_at_once_each_succeed() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("github-tokens/app");

    std::thread::scope(|scope| {
        let saves: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| (0..20).try_for_each(|_| save_github_token(&path, TOKEN))))
            .collect();
        for save in saves {
            save.join().unwrap().unwrap();
        }
    });

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{TOKEN}\n")
    );
    assert_eq!(
        std::fs::read_dir(root.path().join("github-tokens"))
            .unwrap()
            .count(),
        1
    );
}
