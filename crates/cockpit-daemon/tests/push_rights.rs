//! #153, #857 decision 3: the daemon pushes a project's checkout only to a host whose grant
//! carries the owner's push right, bound with `torlek network grant PROJECT HOST --push`.
//! Through the real daemon and CLI binaries; nothing is pushed anywhere.

mod candidate;
mod real_daemon;

use std::path::PathBuf;

use real_daemon::Home;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A home with the project `cockpit`, whose origin is on GitHub.
async fn started() -> (Home, real_daemon::Daemon, PathBuf) {
    let home = Home::new();
    let daemon = home.start();
    with_project(home, daemon).await
}

/// [`started`] with a `gitleaks` running `script` first on the daemon's `PATH`.
async fn started_with_gitleaks(script: &str) -> (Home, real_daemon::Daemon, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let home = Home::new();
    let folder = home.path().join("gitleaks-bin");
    std::fs::create_dir_all(&folder).unwrap();
    let program = folder.join("gitleaks");
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let daemon = home.start_with_path_first(&[folder]);
    with_project(home, daemon).await
}

async fn with_project(
    home: Home,
    daemon: real_daemon::Daemon,
) -> (Home, real_daemon::Daemon, PathBuf) {
    let client = home.connect().await;
    let (root, _) = home.project(&client, "cockpit").await;
    let added = std::process::Command::new("git")
        .current_dir(&root)
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/owner/cockpit.git",
        ])
        .output()
        .unwrap();
    assert!(added.status.success(), "{}", text(&added.stderr));
    (home, daemon, root)
}

async fn succeeds(home: &Home, arguments: &[&str]) -> String {
    let output = home.cockpit(arguments, &[]).await;
    assert!(
        output.status.success(),
        "{arguments:?}: {}",
        text(&output.stderr)
    );
    text(&output.stdout)
}

async fn fails(home: &Home, arguments: &[&str]) -> String {
    let output = home.cockpit(arguments, &[]).await;
    assert!(
        !output.status.success(),
        "{arguments:?}: {}",
        text(&output.stdout)
    );
    text(&output.stderr)
}

#[tokio::test(flavor = "multi_thread")]
async fn the_push_right_is_bound_on_the_projects_grant_of_the_host() {
    let (home, _daemon, _) = started().await;
    succeeds(&home, &["network", "grant", "cockpit", "github.com"]).await;

    let bound = succeeds(
        &home,
        &["network", "grant", "cockpit", "github.com", "--push"],
    )
    .await;

    assert!(
        bound.contains("github.com:443 for project cockpit"),
        "{bound}"
    );
    assert!(
        bound.contains("push: bound by the owner through the CLI"),
        "{bound}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn binding_push_without_the_hosts_grant_is_refused_naming_the_grant_first() {
    let (home, _daemon, _) = started().await;

    let refused = fails(
        &home,
        &["network", "grant", "cockpit", "github.com", "--push"],
    )
    .await;

    assert!(
        refused.contains("`torlek network grant cockpit github.com` first"),
        "{refused}"
    );
    assert_eq!(succeeds(&home, &["network", "grants"]).await, "");
}

#[tokio::test(flavor = "multi_thread")]
async fn binding_push_to_a_host_no_forge_adapter_pushes_to_is_refused() {
    let (home, _daemon, _) = started().await;

    let refused = fails(
        &home,
        &["network", "grant", "cockpit", "api.github.com", "--push"],
    )
    .await;

    assert!(
        refused.contains("no forge adapter pushes to api.github.com"),
        "{refused}"
    );
}

/// The upgrade binds no push right (the owner's decision of 2026-11-22): a project reaching
/// github.com still has its push refused, naming the command that binds it.
#[tokio::test(flavor = "multi_thread")]
async fn a_push_without_the_right_is_refused_naming_the_command_that_binds_it() {
    let (home, _daemon, _) = started().await;
    succeeds(&home, &["network", "grant", "cockpit", "github.com"]).await;

    let refused = fails(&home, &["checkout", "push", "1"]).await;

    assert!(
        refused.contains("`torlek network grant cockpit github.com --push`"),
        "{refused}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn doctor_names_the_command_that_binds_the_push_right() {
    let (home, _daemon, root) = started().await;
    succeeds(&home, &["network", "grant", "cockpit", "github.com"]).await;

    let output = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&output.stdout);
    let mut lines = report.lines();
    let row = lines
        .by_ref()
        .find(|line| line.contains("  push: "))
        .unwrap_or_else(|| panic!("no push requirement in:\n{report}"));
    assert!(row.contains("needs-authorization"), "{row}");
    assert_eq!(
        lines.next().map(str::trim),
        Some("next: `torlek network grant cockpit github.com --push`")
    );
}

/// The doctor row of the project's `push` requirement and the line after it.
async fn push_row(home: &Home, root: &std::path::Path) -> (String, Option<String>) {
    let output = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;
    let report = text(&output.stdout);
    let mut lines = report.lines();
    let row = lines
        .by_ref()
        .find(|line| line.contains("  push: "))
        .unwrap_or_else(|| panic!("no push requirement in:\n{report}"))
        .to_owned();
    (row, lines.next().map(|line| line.trim().to_owned()))
}

/// #694: agents' pushes are scanned by gitleaks, which the `push` row names.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_names_the_gitleaks_that_scans_a_pushing_projects_agents() {
    let (home, _daemon, root) =
        started_with_gitleaks("#!/bin/sh\n[ \"$1\" = version ] && echo 8.30.1\n").await;
    succeeds(&home, &["network", "grant", "cockpit", "github.com"]).await;
    succeeds(
        &home,
        &["network", "grant", "cockpit", "github.com", "--push"],
    )
    .await;

    let (row, _) = push_row(&home, &root).await;

    assert!(row.contains("available"), "{row}");
    assert!(row.contains("gitleaks 8.30.1"), "{row}");
}

/// #694, the owner's decision of 2026-11-23 (option B): a project whose agents push misses
/// gitleaks when it does not run.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_says_a_pushing_project_misses_a_gitleaks_that_does_not_run() {
    let (home, _daemon, root) =
        started_with_gitleaks("#!/bin/sh\necho 'broken install' >&2\nexit 2\n").await;
    succeeds(&home, &["network", "grant", "cockpit", "github.com"]).await;
    succeeds(
        &home,
        &["network", "grant", "cockpit", "github.com", "--push"],
    )
    .await;

    let (row, next) = push_row(&home, &root).await;

    assert!(row.contains("missing-prerequisite"), "{row}");
    assert!(row.contains("broken install"), "{row}");
    assert_eq!(
        next.as_deref(),
        Some("next: install gitleaks on the daemon's PATH")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_is_bound_whatever_its_case_or_default_port() {
    for written in ["GitHub.com", "github.com:443"] {
        let (home, _daemon, _) = started().await;
        succeeds(&home, &["network", "grant", "cockpit", "github.com"]).await;

        let bound = succeeds(&home, &["network", "grant", "cockpit", written, "--push"]).await;

        assert!(bound.contains("push: bound"), "{written}: {bound}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn binding_push_twice_names_the_grant_and_how_to_take_it_back() {
    let (home, _daemon, _) = started().await;
    succeeds(&home, &["network", "grant", "cockpit", "github.com"]).await;
    succeeds(
        &home,
        &["network", "grant", "cockpit", "github.com", "--push"],
    )
    .await;

    let again = fails(
        &home,
        &["network", "grant", "cockpit", "github.com", "--push"],
    )
    .await;

    assert!(again.contains("network grant 1 already"), "{again}");
    assert!(again.contains("`torlek network revoke 1`"), "{again}");
}
