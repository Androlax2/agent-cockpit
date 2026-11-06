//! Report 13 and #64 at the daemon's socket: `version-report` names the release the daemon
//! runs, the schema its database holds and the bundle each live session started with, so
//! "installed, but these sessions still use the previous bundle" is a state the owner can read.

use cockpit_protocol::VersionReportView;
use cockpit_sessions::LaunchBundle;

use super::*;

async fn version_report(client: &Connection) -> VersionReportView {
    match request(client, Command::VersionReport).await {
        Ok(Reply::VersionReport { report }) => *report,
        other => panic!("version-report: {other:?}"),
    }
}

fn previous_bundle() -> LaunchBundle {
    LaunchBundle {
        release: "0.0.1-previous".to_owned(),
        instruction_manifest: "m-0000000000000001".to_owned(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_report_names_the_release_the_daemon_runs_and_the_protocol_it_speaks() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;

    let report = version_report(&client).await;

    assert_eq!(report.release, env!("CARGO_PKG_VERSION"));
    assert!(report.instruction_manifest.starts_with("m-"), "{report:?}");
    assert_eq!(report.protocol_versions, [1]);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_report_shows_every_capabilitys_schema_at_the_version_the_daemon_knows() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;

    let report = version_report(&client).await;

    let owners: Vec<&str> = report
        .schema
        .iter()
        .map(|owner| owner.owner.as_str())
        .collect();
    let expected: Vec<&str> = cockpit_daemon::MIGRATION_SETS
        .iter()
        .map(|set| set.owner)
        .collect();
    assert_eq!(owners, expected);
    for owner in &report.schema {
        assert_eq!(owner.applied, owner.known, "{owner:?}");
        assert!(owner.known >= 1, "{owner:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_shows_the_bundle_its_latest_launch_started_with() {
    let home = Home::new();
    let with_bundle = cockpit_sessions::Settings {
        bundle: Some(previous_bundle()),
        ..settings()
    };
    let daemon = home.start(BASELINE, with_bundle).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let session = new_session(&client, &root, None).await;

    let report = version_report(&client).await;

    assert_eq!(report.sessions.len(), 1, "{report:?}");
    assert_eq!(report.sessions[0].session, session.id);
    assert_eq!(report.sessions[0].state, SessionStateView::Running);
    assert_eq!(
        report.sessions[0].bundle,
        Some(cockpit_protocol::LaunchBundleView {
            release: "0.0.1-previous".to_owned(),
            instruction_manifest: "m-0000000000000001".to_owned(),
        })
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launch_that_recorded_no_bundle_shows_none() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    new_session(&client, &root, None).await;

    let report = version_report(&client).await;

    assert_eq!(report.sessions[0].bundle, None, "{report:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ended_session_is_not_listed() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let root = home.project(&client).await;
    let session = new_session(&client, &root, None).await;
    session_reply(
        &client,
        Command::SessionEnd {
            session: session.id.clone(),
        },
    )
    .await;

    let report = version_report(&client).await;

    assert_eq!(report.sessions, Vec::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_not_read_which_sessions_run_which_bundle() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let agent = Connection::open(daemon.agent_socket_path(), GENEROUS)
        .await
        .unwrap();

    let refused = request(&agent, Command::VersionReport).await;

    assert_eq!(refusal_code(refused), ErrorCode::NotAuthorized);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_capability_the_database_holds_and_the_daemon_does_not_know_shows_as_newer() {
    let home = Home::new();
    let daemon = home.start(BASELINE, settings()).await;
    let client = connect(&daemon).await;
    let database = home.database.lock().unwrap().clone().unwrap();
    let write = database.write().await.unwrap();
    write
        .execute(
            "INSERT INTO cockpit_migrations (owner, version, sql) VALUES ('from-the-future', 3, '')",
            (),
        )
        .await
        .unwrap();
    write.commit().await.unwrap();

    let report = version_report(&client).await;

    let unknown = report
        .schema
        .iter()
        .find(|owner| owner.owner == "from-the-future")
        .unwrap_or_else(|| panic!("{report:?}"));
    assert_eq!((unknown.applied, unknown.known), (3, 0));
}
