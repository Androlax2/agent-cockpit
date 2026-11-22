//! Pushing a checkout's branch only to a host its project may push to (#153, #857 decision 3),
//! through `cockpit_projects::push` on real Git with the push itself recorded, never sent.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cockpit_forge_api::ForgeHost;
use cockpit_projects::{Error, Git, push};
use common::{Scratch, git, register};

/// Real Git for every call but `push`, whose arguments it writes to `pushed` and succeeds.
fn recording_git(scratch: &Scratch) -> (Git, PathBuf) {
    let wrapper = scratch.path().join("git-recording");
    let pushed = scratch.path().join("pushed");
    let quote = |path: &Path| format!("'{}'", path.to_str().unwrap().replace('\'', "'\\''"));
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport HOME={} GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1\n\
             for argument; do\n  if [ \"$argument\" = push ]; then printf '%s\\n' \"$@\" > {}; \
             exit 0; fi\ndone\nexec /usr/bin/git \"$@\"\n",
            quote(&scratch.path().join("git-home")),
            quote(&pushed),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    (Git::with_program(wrapper), pushed)
}

fn github() -> Vec<ForgeHost> {
    vec![ForgeHost::new("github.com").unwrap()]
}

/// Pushes `app`'s main checkout, whose origin is `origin`, allowed `push_hosts`; with the
/// arguments Git was given for the push, `None` when it was not asked to push.
async fn push_with_origin(
    origin: &str,
    push_hosts: Vec<ForgeHost>,
) -> (Result<cockpit_projects::Pushed, Error>, Option<String>) {
    let scratch = Scratch::new();
    let root = scratch.repository("app");
    git(&root, &["remote", "add", "origin", origin]);
    let database = scratch.open().await;
    let (_, checkout) = register(&database, &root).await;
    let (recording, pushed) = recording_git(&scratch);

    let result = push(&database, &recording, checkout.id, push_hosts).await;
    (result, std::fs::read_to_string(pushed).ok())
}

fn refusal(result: Result<cockpit_projects::Pushed, Error>) -> String {
    match result {
        Err(Error::Refused { reason }) => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_branch_is_pushed_to_an_https_origin_on_a_host_the_project_may_push_to() {
    let (result, recorded) = push_with_origin("https://github.com/owner/app.git", github()).await;

    let pushed = result.unwrap();
    assert_eq!(pushed.branch, "main");
    assert_eq!(pushed.remote_url, "https://github.com/owner/app.git");
    let arguments = recorded.unwrap();
    assert!(
        arguments
            .ends_with("push\n--quiet\n--set-upstream\norigin\nrefs/heads/main:refs/heads/main\n"),
        "{arguments}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_without_the_push_right_is_refused_naming_the_command_that_binds_it() {
    for push_hosts in [vec![], vec![ForgeHost::new("forge.example.com").unwrap()]] {
        let (result, recorded) =
            push_with_origin("https://github.com/owner/app.git", push_hosts.clone()).await;

        let reason = refusal(result);
        assert!(
            reason.contains("`torlek network grant app github.com --push`"),
            "{push_hosts:?}: {reason}"
        );
        assert_eq!(recorded, None, "{push_hosts:?}: nothing is pushed");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ssh_origin_is_refused_naming_ssh_even_on_a_host_the_project_may_push_to() {
    for origin in [
        "git@github.com:owner/app.git",
        "ssh://git@github.com/owner/app.git",
    ] {
        let (result, recorded) = push_with_origin(origin, github()).await;

        let reason = refusal(result);
        assert!(reason.contains("SSH"), "{origin}: {reason}");
        assert_eq!(recorded, None, "{origin}: nothing is pushed");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_origin_naming_a_credential_is_refused() {
    let (result, _) = push_with_origin("https://token@github.com/owner/app.git", github()).await;

    let reason = refusal(result);
    assert!(reason.contains("names a user or a password"), "{reason}");
}
