//! `cockpit doctor` against the real daemon binary (#50): the readiness report names each
//! requirement with its status and the next action, from the hosts the daemon found at its
//! start (stand-in Greywall, greyproxy and claude, and a stand-in `docker`), and exits 1
//! only when a requirement blocks.

mod real_daemon;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cockpit_client::Connection;
use cockpit_protocol::{Command, ErrorCode, Reply};
use real_daemon::{GENEROUS, Home};

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The line `doctor` printed for `name`, and the `next:` line under it when there is one.
fn requirement<'report>(report: &'report str, name: &str) -> (&'report str, Option<&'report str>) {
    let mut lines = report.lines();
    let line = lines
        .by_ref()
        .find(|line| line.contains(&format!("  {name}: ")))
        .unwrap_or_else(|| panic!("no {name} requirement in:\n{report}"));
    let next = lines
        .next()
        .and_then(|line| line.trim_start().strip_prefix("next: "));
    (line, next)
}

fn status(line: &str) -> &str {
    line.split_whitespace().next().unwrap()
}

fn fake_docker(home: &Home, info: &str) {
    let program = home.path().join("bin/docker");
    std::fs::write(
        &program,
        format!("#!/bin/sh\n[ \"$1\" = info ] || exit 9\nprintf '%s\\n' '{info}'\n"),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn declare_docker(root: &Path) {
    std::fs::create_dir_all(root.join(".cockpit")).unwrap();
    std::fs::write(
        root.join(".cockpit/config.toml"),
        "[execution]\nenvironment = \"docker\"\n",
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_with_its_programs_reports_them_available_and_exits_0() {
    let home = Home::new();
    let _daemon = home.start();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}{}", text(&doctor.stderr));
    for name in [
        "config",
        "platform",
        "tmux",
        "sandbox",
        "provider claude",
        "resource controls",
    ] {
        let (line, _) = requirement(&report, name);
        assert_eq!(status(line), "available", "{line}");
    }
    let (sandbox, _) = requirement(&report, "sandbox");
    for pin in [
        cockpit_sandbox::PINNED_GREYWALL_COMMIT,
        cockpit_sandbox::PINNED_GREYPROXY_COMMIT,
    ] {
        assert!(sandbox.contains(&pin[..7]), "{sandbox}");
    }
    let (sign_in, next) = requirement(&report, "provider claude sign-in");
    assert_eq!(status(sign_in), "unknown", "{sign_in}");
    assert!(next.is_some(), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_without_greywall_blocks_naming_what_to_install() {
    let home = Home::new();
    let _daemon = home.start_without_greywall();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{report}");
    let (sandbox, next) = requirement(&report, "sandbox");
    assert_eq!(status(sandbox), "missing-prerequisite", "{sandbox}");
    assert!(next.unwrap().contains("greywall"), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unsandboxed_owner_setting_is_reported_loudly_without_blocking() {
    let home = Home::new();
    home.write_owner_config("[agents]\nsandbox = \"off\"\n");
    let _daemon = home.start();

    let doctor = home.cockpit(&["doctor"], &[]).await;

    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}");
    let (sandbox, next) = requirement(&report, "sandbox");
    assert_eq!(status(sandbox), "available", "{sandbox}");
    assert!(sandbox.contains("UNSANDBOXED"), "{sandbox}");
    assert!(next.unwrap().contains("agents.sandbox"), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unregistered_directory_names_the_project_add_command() {
    let home = Home::new();
    let _daemon = home.start();
    let dir = home.path().join("elsewhere");
    std::fs::create_dir_all(&dir).unwrap();

    let doctor = home
        .cockpit(&["doctor", "--project", dir.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{report}");
    let (project, next) = requirement(&report, "project");
    assert_eq!(status(project), "missing-prerequisite", "{project}");
    assert!(
        next.unwrap()
            .contains(&format!("cockpit project add {}", dir.display())),
        "{report}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untrusted_project_needs_authorization_without_blocking() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}");
    let (project, _) = requirement(&report, "project");
    assert_eq!(status(project), "available", "{project}");
    let (trust, next) = requirement(&report, "project trust");
    assert_eq!(status(trust), "needs-authorization", "{trust}");
    assert!(next.unwrap().contains("projects.trusted"), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_docker_project_reports_the_rootless_engine_it_reaches() {
    let home = Home::new();
    fake_docker(
        &home,
        r#"{"ID":"7c2b","Name":"box","ServerVersion":"28.1.1","SecurityOptions":["name=rootless"]}"#,
    );
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    declare_docker(&root);

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert!(doctor.status.success(), "{report}");
    let (docker, _) = requirement(&report, "docker");
    assert_eq!(status(docker), "available", "{docker}");
    assert!(
        docker.contains("engine 7c2b on box (Docker 28.1.1), rootless"),
        "{docker}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_docker_project_on_a_rootful_engine_blocks() {
    let home = Home::new();
    fake_docker(
        &home,
        r#"{"ID":"7c2b","Name":"box","ServerVersion":"28.1.1","SecurityOptions":["name=seccomp,profile=builtin"]}"#,
    );
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;
    declare_docker(&root);

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert_eq!(doctor.status.code(), Some(1), "{report}");
    let (docker, next) = requirement(&report, "docker");
    assert_eq!(status(docker), "missing-prerequisite", "{docker}");
    assert!(next.unwrap().contains("rootless"), "{report}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_project_that_does_not_need_docker_never_runs_it() {
    let home = Home::new();
    let probed = home.path().join("docker-ran");
    let program = home.path().join("bin/docker");
    std::fs::write(
        &program,
        format!("#!/bin/sh\ntouch '{}'\n", probed.display()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let doctor = home
        .cockpit(&["doctor", "--project", root.to_str().unwrap()], &[])
        .await;

    let report = text(&doctor.stdout);
    assert!(!report.contains("  docker: "), "{report}");
    assert!(!probed.exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relative_project_directory_is_refused_on_the_wire() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;

    let refused = client
        .request(
            Command::Readiness {
                project: Some("projects/app".to_owned()),
            },
            GENEROUS,
        )
        .await;

    let error = real_daemon_refusal(refused);
    assert_eq!(error.code, ErrorCode::InvalidArgument);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_not_ask_for_the_owners_readiness_report() {
    let home = Home::new();
    let _daemon = home.start();

    let agent = Connection::open(&home.agent_socket_path(), GENEROUS)
        .await
        .expect("the agent socket accepts the handshake");
    let refused = agent
        .request(Command::Readiness { project: None }, GENEROUS)
        .await;

    let error = real_daemon_refusal(refused);
    assert_eq!(error.code, ErrorCode::NotAuthorized, "{error:?}");
}

fn real_daemon_refusal(
    reply: Result<Reply, cockpit_client::ClientError>,
) -> cockpit_protocol::ProtocolError {
    match reply {
        Err(cockpit_client::ClientError::Daemon(error)) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}
