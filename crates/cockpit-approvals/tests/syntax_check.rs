//! `Runner::check_syntax` against stand-in shells that record being executed (#494): a
//! command reaches a shell before approval only when the program the shell path resolves
//! to is known to parse without running under `-n`.
#![cfg(target_os = "linux")]

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use cockpit_approvals::{RunSettings, Runner, Syntax};
use cockpit_processes::{LaunchContainment, Supervisor, SystemdTools};

/// A program at `<dir>/<name>` that appends its arguments to `<dir>/executed` and succeeds.
fn recording_program(dir: &Path, name: &str) -> PathBuf {
    let program = dir.join(name);
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
        dir.join("executed").display()
    );
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    program
}

fn executed(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("executed")).unwrap_or_default()
}

fn runner_with_shell(dir: &Path, shell: PathBuf) -> Runner {
    let settings = RunSettings::new(shell, PathBuf::from("/bin/true"), Vec::new());
    let supervisor = Supervisor::new(
        dir.join("executions"),
        LaunchContainment::ProcessGroup,
        SystemdTools::from_path(),
    );
    Runner::new(settings, supervisor)
}

#[tokio::test]
async fn a_shell_named_like_a_known_one_but_resolving_to_another_is_not_given_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let nushell = recording_program(dir.path(), "nu");
    let named_bash = dir.path().join("bash");
    symlink(&nushell, &named_bash).unwrap();
    let runner = runner_with_shell(dir.path(), named_bash);

    let checked = runner.check_syntax("echo unapproved").await.unwrap();

    assert_eq!(checked, Ok(Syntax::NotChecked));
    assert_eq!(executed(dir.path()), "");
}

#[tokio::test]
async fn a_link_to_a_known_shell_is_given_the_command_to_parse_only() {
    let dir = tempfile::tempdir().unwrap();
    let bash = recording_program(dir.path(), "bash");
    let login_shell = dir.path().join("login-shell");
    symlink(&bash, &login_shell).unwrap();
    let runner = runner_with_shell(dir.path(), login_shell);

    let checked = runner.check_syntax("echo asked").await.unwrap();

    assert_eq!(checked, Ok(Syntax::Parsed));
    assert_eq!(executed(dir.path()), "-n -c echo asked\n");
}

#[tokio::test]
async fn a_shell_that_cannot_be_resolved_is_a_check_that_could_not_be_made() {
    let dir = tempfile::tempdir().unwrap();
    let runner = runner_with_shell(dir.path(), dir.path().join("missing"));

    let checked = runner.check_syntax("true").await;

    assert!(checked.is_err(), "{checked:?}");
}

#[tokio::test]
async fn the_check_starts_the_program_the_link_resolved_to() {
    let dir = tempfile::tempdir().unwrap();
    let bash = dir.path().join("bash");
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$0\" >> '{}'\n",
        dir.path().join("executed").display()
    );
    std::fs::write(&bash, script).unwrap();
    std::fs::set_permissions(&bash, std::fs::Permissions::from_mode(0o755)).unwrap();
    let login_shell = dir.path().join("login-shell");
    symlink(&bash, &login_shell).unwrap();
    let runner = runner_with_shell(dir.path(), login_shell);

    runner.check_syntax("true").await.unwrap().unwrap();

    assert_eq!(executed(dir.path()), format!("{}\n", bash.display()));
}

#[tokio::test]
async fn changing_an_alias_cannot_redirect_a_syntax_check_to_an_unknown_shell() {
    let dir = tempfile::tempdir().unwrap();
    let bash = recording_program(dir.path(), "bash");
    let link = dir.path().join("login-shell");
    symlink(&bash, &link).unwrap();
    let runner = runner_with_shell(dir.path(), link.clone());
    std::fs::remove_file(&link).unwrap();
    symlink(recording_program(dir.path(), "nu"), &link).unwrap();

    let checked = runner.check_syntax("unapproved command").await.unwrap();

    assert_eq!(checked, Ok(Syntax::NotChecked));
    assert!(!dir.path().join("executed").exists());
}

#[test]
fn a_shell_with_no_known_parse_only_mode_is_the_unchecked_shell() {
    let dir = tempfile::tempdir().unwrap();
    let nushell = recording_program(dir.path(), "nu");

    let runner = runner_with_shell(dir.path(), nushell.clone());

    assert_eq!(runner.unchecked_shell(), Some(nushell.as_path()));
}

#[test]
fn a_known_parse_only_shell_is_not_the_unchecked_shell() {
    let dir = tempfile::tempdir().unwrap();
    let bash = recording_program(dir.path(), "bash");

    let runner = runner_with_shell(dir.path(), bash);

    assert_eq!(runner.unchecked_shell(), None);
}
