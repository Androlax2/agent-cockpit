//! Two Cockpits on one machine (#196, #134): each profile's daemon, sessions and agents keep
//! to `cockpit-<name>` in every XDG base and to their own tmux server, through the real daemon
//! binary, a stand-in Claude Code and the real tmux.

mod real_daemon;

use std::path::Path;
use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_protocol::{Command, Reply};
use real_daemon::{
    BUILD_PROFILE_DIR, GENEROUS, Home, REPORT_PROJECT, REPORTED_LABEL, cockpit_program, new_session,
};

/// Waits until the daemon behind `client` holds `session`'s report from its agent.
async fn wait_for_report(home: &Home, client: &Connection, session: &str) {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let reports = match client.request(Command::AgentReports, GENEROUS).await {
            Ok(Reply::AgentReports { reports }) => reports,
            other => panic!("agent-reports answered {other:?}"),
        };
        if reports.iter().any(|report| {
            report.session == session && report.label.as_deref() == Some(REPORTED_LABEL)
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the agent's report did not land; it printed {:?}",
            std::fs::read_to_string(home.report_log()).unwrap_or_default()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The session ids the daemon behind `client` lists.
async fn listed_ids(client: &Connection) -> Vec<String> {
    match client
        .request(Command::SessionList { ended: true }, GENEROUS)
        .await
    {
        Ok(Reply::Sessions { sessions }) => {
            sessions.into_iter().map(|session| session.id).collect()
        }
        other => panic!("session-list answered {other:?}"),
    }
}

/// What each XDG base and the private tmux directory of the home hold, by name.
fn profile_entries(home: &Home) -> Vec<String> {
    let mut entries = Vec::new();
    for base in ["run", "state", "config", "cache"] {
        for entry in std::fs::read_dir(home.path().join(base))
            .into_iter()
            .flatten()
        {
            let name = entry.unwrap().file_name();
            entries.push(format!("{base}/{}", name.to_string_lossy()));
        }
    }
    let tmux = home.path().join("tmux");
    for user_dir in std::fs::read_dir(&tmux).unwrap() {
        for server in std::fs::read_dir(user_dir.unwrap().path()).unwrap() {
            let name = server.unwrap().file_name();
            entries.push(format!("tmux server {}", name.to_string_lossy()));
        }
    }
    entries.sort();
    entries
}

fn assert_cockpit_built(cockpit: &Path) {
    assert!(
        cockpit.is_file(),
        "{} is missing: run the workspace's tests (`make crates-test`), which build it",
        cockpit.display()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_of_a_profile_reports_to_its_daemon_and_leaves_the_legacy_layout_untouched() {
    assert_cockpit_built(&cockpit_program());
    let home = Home::new();
    let legacy = legacy_layout(&home);
    // `next` is neither this build's profile nor the default, so the agent's `cockpit` (this
    // build) reaches the `next` daemon only through what its launch carries.
    let _next = home.start_profile("next");
    let client = home.connect_to("cockpit-next").await;
    let (root, _) = home.project(&client, REPORT_PROJECT).await;
    let session = new_session(&client, &root).await;

    wait_for_report(&home, &client, &session.id).await;

    assert_eq!(
        profile_entries(&home),
        [
            "config/cockpit",
            "run/cockpit",
            "run/cockpit-next",
            "state/cockpit",
            "state/cockpit-next",
            "tmux server cockpit-next",
        ]
    );
    for (path, text) in legacy {
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            text,
            "{}",
            path.display()
        );
    }
}

/// The legacy Cockpit's files in `home` (#134), with what each holds.
fn legacy_layout(home: &Home) -> Vec<(std::path::PathBuf, &'static str)> {
    let files = [
        (
            "config/cockpit/config.toml",
            "# the legacy Cockpit's settings\n",
        ),
        (
            "state/cockpit/sessions.json",
            "{\"sessions\": [\"legacy\"]}\n",
        ),
        ("run/cockpit/ipc.sock.lock", "legacy\n"),
    ];
    files
        .into_iter()
        .map(|(relative, text)| {
            let path = home.path().join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            (path, text)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_default_profile_and_a_dev_build_run_side_by_side_each_with_its_own_sessions() {
    assert_cockpit_built(&cockpit_program());
    let home = Home::new();
    let _installed = home.start_profile("cockpit");
    let _dev = home.start();
    let installed = home.connect_to("cockpit").await;
    let dev = home.connect().await;

    let (root, _) = home.project(&dev, REPORT_PROJECT).await;
    let dev_session = new_session(&dev, &root).await;
    wait_for_report(&home, &dev, &dev_session.id).await;
    let (other_root, _) = home.project(&installed, "installed").await;
    let installed_session = new_session(&installed, &other_root).await;

    assert_eq!(
        listed_ids(&dev).await,
        std::slice::from_ref(&dev_session.id)
    );
    assert_eq!(
        listed_ids(&installed).await,
        std::slice::from_ref(&installed_session.id)
    );
    assert_eq!(
        profile_entries(&home),
        [
            "run/cockpit".to_owned(),
            format!("run/{BUILD_PROFILE_DIR}"),
            "state/cockpit".to_owned(),
            format!("state/{BUILD_PROFILE_DIR}"),
            "tmux server cockpit".to_owned(),
            format!("tmux server {BUILD_PROFILE_DIR}"),
        ]
    );
}
