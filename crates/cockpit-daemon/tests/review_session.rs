//! #60: `torlek session review PROJECT PULL_REQUEST` starts a session in a review clone of the
//! pull request's head, whose first message asks for a review. Through the real daemon and
//! CLI; the forge is a local bare repository holding `refs/pull/7/head`, so nothing reaches
//! GitHub.
#![cfg(target_os = "linux")]

mod candidate;
mod real_daemon;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command as Process;

use real_daemon::{Home, cockpit_program, listed, wait_for_running};

fn assert_the_cli_is_built() {
    let cockpit = cockpit_program();
    assert!(
        cockpit.is_file(),
        "{} is missing: run the workspace's tests, which build it",
        cockpit.display()
    );
}

fn git(dir: &Path, arguments: &[&str]) -> String {
    let output = Process::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Fixture", "-c", "user.email=f@example.com"])
        .args(arguments)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {arguments:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A `git` first on the daemon's `PATH` that lets `fetch` read the local bare forge: it puts
/// `protocol.file.allow=always` after the caller's leading `-c` pairs, where it wins over the
/// host Git safety rules' own setting. Every other call runs Git unchanged.
fn git_allowing_a_local_forge(home: &Home) {
    let real = String::from_utf8(
        Process::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let bin: PathBuf = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join("git");
    std::fs::write(
        &script,
        format!(
            r#"#!/bin/sh
remaining=$#
inserted=no
while [ "$remaining" -gt 0 ]; do
    argument=$1
    shift
    remaining=$((remaining - 1))
    if [ "$inserted" = no ] && [ "$argument" = -c ] && [ "$remaining" -gt 0 ]; then
        set -- "$@" -c "$1"
        shift
        remaining=$((remaining - 1))
        continue
    fi
    if [ "$inserted" = no ]; then
        [ "$argument" = fetch ] && set -- "$@" -c protocol.file.allow=always
        inserted=yes
    fi
    set -- "$@" "$argument"
done
exec '{}' "$@"
"#,
            real.trim()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A bare `forge` holding the project's `main` and, as pull request 7, a commit on top of
/// it; the project's `origin`. Returns the pull request's head.
fn forge_with_pull_request_7(home: &Home, root: &Path) -> String {
    let forge = home.path().join("forge.git");
    git(
        home.path().as_path(),
        &["init", "--quiet", "--bare", forge.to_str().unwrap()],
    );
    git(root, &["remote", "add", "origin", forge.to_str().unwrap()]);
    git(root, &["push", "--quiet", "origin", "main"]);
    let contributor = home.path().join("contributor");
    git(
        home.path().as_path(),
        &[
            "clone",
            "--quiet",
            forge.to_str().unwrap(),
            contributor.to_str().unwrap(),
        ],
    );
    std::fs::write(contributor.join("CHANGE"), "proposed\n").unwrap();
    git(&contributor, &["add", "."]);
    git(
        &contributor,
        &["commit", "--quiet", "-m", "proposed change"],
    );
    git(
        &contributor,
        &["push", "--quiet", "origin", "HEAD:refs/pull/7/head"],
    );
    git(&contributor, &["rev-parse", "HEAD"])
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_starts_a_review_session_in_a_clone_of_the_pull_requests_head() {
    assert_the_cli_is_built();
    let home = Home::new();
    git_allowing_a_local_forge(&home);
    let _daemon = home.start();
    let owner = home.connect().await;
    let (root, project) = home.project(&owner, "app").await;
    let proposed = forge_with_pull_request_7(&home, &root);

    let output = home
        .cockpit(
            &["session", "review", &project, "7", "--agent", "claude"],
            &[],
        )
        .await;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}{stderr}");
    let session = stdout.split('\t').next().unwrap().to_owned();
    wait_for_running(&owner, &session).await;
    let started = listed(&owner, &session).await;
    let clone = Path::new(&started.working_dir);
    assert!(clone.ends_with(".parallel/app/pr-7"), "{}", clone.display());
    assert_eq!(git(clone, &["rev-parse", "HEAD"]), proposed);
    assert!(
        stderr.contains(&format!("review clone {} of app#7", started.checkout)),
        "{stderr}"
    );
    home.wait_for_launches(&session, 1).await;
    let words = home.arguments_of(&session);
    assert!(
        words[0].contains("Review pull request #7 ") && words[0].contains("Do not push"),
        "{words:?}"
    );
}
