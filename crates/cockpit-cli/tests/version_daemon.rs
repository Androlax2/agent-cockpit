//! `torlek version --all` and the running daemon (report 13, #64), driven as a process against
//! a scripted daemon socket. The daemon's own report is proven in
//! `cockpit-daemon/tests/sessions/version_report.rs`; these prove what the owner reads.
mod scripted;

use std::os::unix::fs::DirBuilderExt;
use std::path::PathBuf;
use std::process::Output;

use cockpit_protocol::{
    Command, ErrorCode, InstructionTopicView, LaunchBundleView, Outcome, ProtocolError, Reply,
    SchemaOwnerView, SessionBundleView, SessionStateView, VersionReportView,
};
use scripted::{scripted_conversation, text};

const INSTALLED: &str = env!("CARGO_PKG_VERSION");

struct Home {
    dir: tempfile::TempDir,
}

impl Home {
    fn new() -> Home {
        Home {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn daemon_socket(&self) -> PathBuf {
        let path = self.dir.path().join("run/torlek-dev/daemon.sock");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path.parent().unwrap())
            .unwrap();
        path
    }

    async fn version_all(&self) -> Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_torlek"));
        command
            .args(["version", "--all"])
            .env_clear()
            .env("HOME", self.dir.path())
            .env("XDG_RUNTIME_DIR", self.dir.path().join("run"));
        tokio::task::spawn_blocking(move || command.output().unwrap())
            .await
            .unwrap()
    }
}

fn bundle(release: &str, manifest: &str) -> LaunchBundleView {
    LaunchBundleView {
        release: release.to_owned(),
        instruction_manifest: manifest.to_owned(),
    }
}

fn report(release: &str) -> VersionReportView {
    VersionReportView {
        release: release.to_owned(),
        source_revision: Some("0123abcd".to_owned()),
        instruction_manifest: "m-00000000000000aa".to_owned(),
        protocol_versions: vec![1],
        schema: vec![
            SchemaOwnerView {
                owner: "sessions".to_owned(),
                applied: 13,
                known: 13,
            },
            SchemaOwnerView {
                owner: "tasks".to_owned(),
                applied: 4,
                known: 4,
            },
        ],
        sessions: Vec::new(),
    }
}

fn reply(report: VersionReportView) -> Outcome {
    Outcome::Ok(Reply::VersionReport {
        report: Box::new(report),
    })
}

fn lines(output: &Output) -> Vec<String> {
    assert!(output.status.success(), "{}", text(&output.stderr));
    text(&output.stdout).lines().map(str::to_owned).collect()
}

#[tokio::test]
async fn it_names_the_daemons_release_manifest_and_protocol_after_the_existing_lines() {
    let home = Home::new();
    let daemon = scripted_conversation(&home.daemon_socket(), vec![reply(report(INSTALLED))]);

    let output = home.version_all().await;

    let printed = lines(&output);
    assert!(
        printed[0].starts_with(&format!("torlek {INSTALLED} (")),
        "{printed:?}"
    );
    assert_eq!(printed[1], "profile dev (development build default)");
    assert_eq!(printed[2], "build development");
    assert!(
        printed.contains(&format!(
            "daemon: release {INSTALLED} (revision 0123abcd), protocol 1"
        )),
        "{printed:?}"
    );
    assert!(
        printed.contains(&"daemon: instruction manifest m-00000000000000aa".to_owned()),
        "{printed:?}"
    );
    assert_eq!(daemon.await.unwrap(), [Command::VersionReport]);
}

#[tokio::test]
async fn a_daemon_of_the_installed_release_gets_no_restart_hint() {
    let home = Home::new();
    scripted_conversation(&home.daemon_socket(), vec![reply(report(INSTALLED))]);

    let printed = lines(&home.version_all().await);

    assert!(
        !printed
            .iter()
            .any(|line| line.contains("stop and start it")),
        "{printed:?}"
    );
}

#[tokio::test]
async fn a_daemon_of_another_release_than_the_installed_one_says_to_restart_it() {
    let home = Home::new();
    scripted_conversation(&home.daemon_socket(), vec![reply(report("0.0.1-previous"))]);

    let printed = lines(&home.version_all().await);

    assert!(
        printed.contains(&format!(
            "daemon: runs 0.0.1-previous, not the installed {INSTALLED}; stop and start it to use the installed release"
        )),
        "{printed:?}"
    );
}

#[tokio::test]
async fn a_schema_every_capability_holds_at_a_known_version_is_compatible() {
    let home = Home::new();
    scripted_conversation(&home.daemon_socket(), vec![reply(report(INSTALLED))]);

    let printed = lines(&home.version_all().await);

    assert!(
        printed.contains(
            &"schema: compatible, 2 capabilities at versions the daemon knows".to_owned()
        ),
        "{printed:?}"
    );
}

#[tokio::test]
async fn a_capability_newer_than_the_daemon_knows_is_incompatible() {
    let home = Home::new();
    let mut newer = report(INSTALLED);
    newer.schema[1].applied = 5;
    scripted_conversation(&home.daemon_socket(), vec![reply(newer)]);

    let printed = lines(&home.version_all().await);

    assert!(
        printed
            .contains(&"schema: INCOMPATIBLE, tasks is at 5 where the daemon knows 4".to_owned()),
        "{printed:?}"
    );
}

#[tokio::test]
async fn each_live_session_says_whether_it_runs_the_daemons_bundle() {
    let home = Home::new();
    let mut with_sessions = report(INSTALLED);
    with_sessions.sessions = vec![
        SessionBundleView {
            session: "s1".to_owned(),
            state: SessionStateView::Running,
            bundle: Some(bundle(INSTALLED, "m-00000000000000aa")),
        },
        SessionBundleView {
            session: "s2".to_owned(),
            state: SessionStateView::Running,
            bundle: Some(bundle("0.0.1-previous", "m-00000000000000bb")),
        },
        SessionBundleView {
            session: "s3".to_owned(),
            state: SessionStateView::Launching,
            bundle: None,
        },
    ];
    scripted_conversation(&home.daemon_socket(), vec![reply(with_sessions)]);

    let printed = lines(&home.version_all().await);

    assert!(
        printed.contains(&format!(
            "session s1 (running): helpers {INSTALLED}, instructions m-00000000000000aa: the running daemon's bundle"
        )),
        "{printed:?}"
    );
    assert!(
        printed.contains(&"session s2 (running): helpers 0.0.1-previous, instructions m-00000000000000bb: previous bundle".to_owned()),
        "{printed:?}"
    );
    assert!(
        printed.contains(&"session s3 (launching): bundle not recorded".to_owned()),
        "{printed:?}"
    );
}

#[tokio::test]
async fn no_live_session_is_said_plainly() {
    let home = Home::new();
    scripted_conversation(&home.daemon_socket(), vec![reply(report(INSTALLED))]);

    let printed = lines(&home.version_all().await);

    assert!(
        printed.contains(&"sessions: none running".to_owned()),
        "{printed:?}"
    );
}

#[tokio::test]
async fn no_daemon_is_a_normal_state_that_still_succeeds() {
    let home = Home::new();

    let printed = lines(&home.version_all().await);

    assert!(
        printed
            .iter()
            .any(|line| line.starts_with("daemon: not running (cannot reach ")),
        "{printed:?}"
    );
    assert!(
        printed.contains(&"schema: not checked; the daemon opens the database".to_owned()),
        "{printed:?}"
    );
}

#[tokio::test]
async fn a_daemon_that_predates_version_reports_still_names_its_release() {
    let home = Home::new();
    let unknown = Outcome::Error(ProtocolError::new(
        ErrorCode::UnknownCommand,
        "this daemon does not know the command `version-report`".to_owned(),
    ));
    let catalog = Outcome::Ok(Reply::InstructionCatalog {
        version: "0.0.1-previous".to_owned(),
        manifest: "m-00000000000000bb".to_owned(),
        topics: Vec::<InstructionTopicView>::new(),
    });
    let daemon = scripted_conversation(&home.daemon_socket(), vec![unknown, catalog]);

    let printed = lines(&home.version_all().await);

    assert!(
        printed.iter().any(|line| line.starts_with(
            "daemon: release 0.0.1-previous, instruction manifest m-00000000000000bb; it predates version reports"
        )),
        "{printed:?}"
    );
    assert!(
        printed
            .iter()
            .any(|line| line.contains("stop and start it")),
        "{printed:?}"
    );
    assert_eq!(
        daemon.await.unwrap(),
        [Command::VersionReport, Command::InstructionCatalog]
    );
}

#[tokio::test]
async fn a_refusal_exits_unsuccessfully_after_the_lines_already_printed() {
    let home = Home::new();
    scripted_conversation(
        &home.daemon_socket(),
        vec![Outcome::Error(ProtocolError::new(
            ErrorCode::Internal,
            "the daemon failed".to_owned(),
        ))],
    );

    let output = home.version_all().await;

    assert_eq!(output.status.code(), Some(1), "{}", text(&output.stderr));
    assert!(
        text(&output.stdout).starts_with("torlek "),
        "{}",
        text(&output.stdout)
    );
    assert!(
        text(&output.stderr).contains("the daemon failed"),
        "{}",
        text(&output.stderr)
    );
}

#[tokio::test]
async fn inside_a_session_it_says_the_report_is_the_owners() {
    let home = Home::new();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_torlek"));
    command
        .args(["version", "--all"])
        .env_clear()
        .env("HOME", home.dir.path())
        .env("XDG_RUNTIME_DIR", home.dir.path().join("run"))
        .env("TORLEK_SESSION", "s1");
    let output = tokio::task::spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap();

    let printed = lines(&output);

    assert!(
        printed.contains(&"daemon: not asked from inside a session; run `torlek version --all` in your own terminal".to_owned()),
        "{printed:?}"
    );
}
