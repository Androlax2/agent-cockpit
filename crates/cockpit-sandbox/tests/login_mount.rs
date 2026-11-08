//! A login account's sign-in in a sandboxed session (#59, #858): bound read-write in the
//! session's own home, so a sign-in the harness refreshes reaches the account. Claude Code
//! rewrites its file in place and refuses a link there, so the file is bound at its path;
//! Codex renames a new file over the link's target, so the account's folder is bound and a
//! link names the login in it.

mod common;

use std::path::{Path, PathBuf};

use cockpit_sandbox::{
    AgentSandbox, HostPaths, LoginMount, Platform, Prepared, Refusal, Sandbox, SessionLogin,
};
use common::{launch_request, native_backend, no_host_paths};

fn state_sandbox(state: &Path) -> Sandbox {
    Sandbox::new(
        Platform::Linux,
        HostPaths {
            mount_sources: state.join("mounts"),
            homes: state.join("homes"),
            logins: state.join("logins"),
            ..no_host_paths()
        },
        native_backend(),
    )
}

fn login(state: &Path, provider: &str, file: &str, mount: LoginMount) -> SessionLogin {
    SessionLogin {
        folder: state
            .join("logins")
            .join(provider)
            .join("work")
            .join("login"),
        file: file.to_owned(),
        mount,
    }
}

fn claude_login(state: &Path) -> SessionLogin {
    login(
        state,
        "claude",
        "~/.claude/.credentials.json",
        LoginMount::File,
    )
}

fn codex_login(state: &Path) -> SessionLogin {
    login(
        state,
        "codex",
        "~/.codex/auth.json",
        LoginMount::FolderAndLink,
    )
}

fn prepare(setting: AgentSandbox, state: &Path, login: SessionLogin) -> Result<Prepared, Refusal> {
    let mut request = launch_request("s1", "/home/owner/app");
    request.login = Some(login);
    state_sandbox(state).prepare(
        setting,
        &request,
        PathBuf::from("/usr/bin/claude"),
        Vec::new(),
    )
}

/// The session's own home folder, where the launcher shows `/home/owner`.
fn home_source(state: &Path) -> PathBuf {
    state.join("homes").join("s1")
}

#[test]
fn a_login_rewritten_in_place_is_bound_as_the_file_at_the_harness_path() {
    let state = tempfile::tempdir().unwrap();

    let prepared = prepare(
        AgentSandbox::Required,
        state.path(),
        claude_login(state.path()),
    )
    .unwrap();

    let login = prepared.staged.unwrap().spec.login.unwrap();
    assert_eq!(
        login.source,
        state
            .path()
            .join("logins/claude/work/login/.credentials.json")
    );
    assert_eq!(
        login.destination,
        PathBuf::from("/home/owner/.claude/.credentials.json")
    );
}

#[test]
fn a_login_replaced_beside_its_link_target_is_bound_as_its_folder() {
    let state = tempfile::tempdir().unwrap();

    let prepared = prepare(
        AgentSandbox::Required,
        state.path(),
        codex_login(state.path()),
    )
    .unwrap();

    let login = prepared.staged.unwrap().spec.login.unwrap();
    assert_eq!(login.source, state.path().join("logins/codex/work/login"));
    assert_eq!(
        login.destination,
        PathBuf::from("/home/owner/.torlek-login")
    );
}

#[test]
fn staging_a_folder_login_links_the_harness_path_to_the_login_in_the_bound_folder() {
    let state = tempfile::tempdir().unwrap();
    let prepared = prepare(
        AgentSandbox::Required,
        state.path(),
        codex_login(state.path()),
    )
    .unwrap();

    prepared.staged.unwrap().write().unwrap();

    let home = home_source(state.path());
    assert_eq!(
        std::fs::read_link(home.join(".codex/auth.json")).unwrap(),
        PathBuf::from("/home/owner/.torlek-login/auth.json")
    );
    assert!(home.join(".torlek-login").is_dir());
}

#[test]
fn staging_a_file_login_replaces_a_link_the_last_launch_left_at_its_path() {
    let state = tempfile::tempdir().unwrap();
    let file = home_source(state.path()).join(".claude/.credentials.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("/home/owner/app/stolen", &file).unwrap();
    let prepared = prepare(
        AgentSandbox::Required,
        state.path(),
        claude_login(state.path()),
    )
    .unwrap();

    prepared.staged.unwrap().write().unwrap();

    let entry = std::fs::symlink_metadata(&file).unwrap();
    assert!(entry.is_file(), "{entry:?}");
}

#[test]
fn staging_a_login_under_a_folder_the_last_launch_made_a_link_fails() {
    let state = tempfile::tempdir().unwrap();
    let home = home_source(state.path());
    std::fs::create_dir_all(&home).unwrap();
    std::os::unix::fs::symlink(state.path(), home.join(".claude")).unwrap();
    let prepared = prepare(
        AgentSandbox::Required,
        state.path(),
        claude_login(state.path()),
    )
    .unwrap();

    let written = prepared.staged.unwrap().write();

    let error = written.unwrap_err().to_string();
    assert!(error.contains("not a folder"), "{error}");
}

#[test]
fn the_evidence_names_the_bound_login() {
    let state = tempfile::tempdir().unwrap();

    let prepared = prepare(
        AgentSandbox::Required,
        state.path(),
        claude_login(state.path()),
    )
    .unwrap();

    let rules: Vec<String> = prepared
        .evidence
        .rules
        .iter()
        .map(|applied| applied.rule.clone())
        .collect();
    assert!(
        rules
            .iter()
            .any(|rule| rule.starts_with("login account's sign-in ")
                && rule.contains("at /home/owner/.claude/.credentials.json, read-write")),
        "{rules:?}"
    );
}

#[test]
fn an_unsandboxed_launch_is_refused_a_login() {
    let state = tempfile::tempdir().unwrap();

    let refused = prepare(AgentSandbox::Off, state.path(), claude_login(state.path())).unwrap_err();

    assert_eq!(refused.rule, "login account");
}
