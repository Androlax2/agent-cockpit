//! #754: a project's GitHub token file in the owner's configuration directory, read so that
//! a wrong file is refused and its contents never reach an error.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_config::{GithubTokenError, read_github_token};

const TOKEN: &str = "github_pat_11AAAAAAA0abcdefghijklmnopqrstuvwxyz0123456789";

fn token_file(dir: &Path, contents: &str, mode: u32) -> PathBuf {
    let path = dir.join("app");
    std::fs::write(&path, contents).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    path
}

#[test]
fn no_file_means_the_project_has_no_token() {
    let dir = tempfile::tempdir().unwrap();

    assert!(
        read_github_token(&dir.path().join("app"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn an_owner_only_file_gives_its_token_without_the_trailing_newline() {
    let dir = tempfile::tempdir().unwrap();
    let path = token_file(dir.path(), &format!("{TOKEN}\n"), 0o600);

    let token = read_github_token(&path).unwrap().unwrap();

    assert_eq!(token.expose(), TOKEN);
}

#[test]
fn a_tokens_debug_form_holds_no_value() {
    let dir = tempfile::tempdir().unwrap();
    let path = token_file(dir.path(), TOKEN, 0o400);

    let token = read_github_token(&path).unwrap().unwrap();

    assert!(!format!("{token:?}").contains(TOKEN));
}

#[test]
fn a_file_with_any_mode_but_0600_or_0400_is_refused_naming_the_fix() {
    for mode in [0o644, 0o640, 0o604, 0o700, 0o500, 0o4600] {
        let dir = tempfile::tempdir().unwrap();
        let path = token_file(dir.path(), TOKEN, mode);

        let error = read_github_token(&path).unwrap_err();

        assert!(
            matches!(error, GithubTokenError::TooOpen { mode: refused, .. } if refused == mode),
            "{mode:o}: {error:?}"
        );
        assert!(error.to_string().contains("chmod 600"), "{error}");
    }
}

#[test]
fn a_symbolic_link_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let target = token_file(dir.path(), TOKEN, 0o600);
    let link = dir.path().join("linked");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let error = read_github_token(&link).unwrap_err();

    assert!(
        matches!(error, GithubTokenError::NotAFile { .. }),
        "{error}"
    );
}

#[test]
fn a_folder_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("app")).unwrap();

    let error = read_github_token(&dir.path().join("app")).unwrap_err();

    assert!(
        matches!(error, GithubTokenError::NotAFile { .. }),
        "{error}"
    );
}

#[test]
fn malformed_files_are_refused_without_their_contents_in_the_error() {
    let cases = [
        ("", "empty"),
        ("\n", "empty"),
        (&*format!("{TOKEN} second-secret-word"), "space"),
        (&*format!("{TOKEN}\nsecond-secret-word\n"), "two lines"),
        (&*format!("GH_TOKEN={TOKEN}"), "a variable assignment"),
    ];
    for (contents, case) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = token_file(dir.path(), contents, 0o600);

        let error = read_github_token(&path).unwrap_err();

        assert!(
            matches!(error, GithubTokenError::Malformed { .. }),
            "{case}: {error:?}"
        );
        let shown = format!("{error} {error:?}");
        assert!(
            !shown.contains(TOKEN) && !shown.contains("second-secret-word"),
            "{case}: {shown}"
        );
    }
}

#[test]
fn a_file_larger_than_any_token_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = token_file(dir.path(), &"a".repeat(4096), 0o600);

    let error = read_github_token(&path).unwrap_err();

    assert!(
        matches!(error, GithubTokenError::Malformed { .. }),
        "{error}"
    );
}
