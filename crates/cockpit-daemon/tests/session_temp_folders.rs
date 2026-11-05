//! #766: each session's agent gets a temporary folder of its own on the main disk, under the
//! daemon's state directory, instead of the host's shared `/tmp`. Through the real daemon
//! binary and a stand-in Claude Code that logs its `TMPDIR` and `CLAUDE_CODE_TMPDIR`.

mod candidate;
mod real_daemon;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use cockpit_client::Connection;
use cockpit_protocol::{Command, Reply, SessionStateView, TargetSizeView};
use real_daemon::{GENEROUS, Home, new_session, wait_for_running, wait_for_state};

/// Starts a session in a project of its own, named `project`, and waits for its agent.
async fn launched_session(home: &Home, client: &Connection, project: &str) -> String {
    let (root, _) = home.project(client, project).await;
    let session = new_session(client, &root).await;
    home.wait_for_launches(&session.id, 1).await;
    wait_for_running(client, &session.id).await;
    session.id
}

/// The `TMPDIR` and `CLAUDE_CODE_TMPDIR` session `id`'s agent logged at its start.
async fn logged_temp_variables(home: &Home, id: &str) -> (String, String) {
    let deadline = Instant::now() + GENEROUS;
    loop {
        let log = std::fs::read_to_string(home.temp_log()).unwrap_or_default();
        let logged = log.lines().find_map(|line| {
            let mut words = line.split(' ');
            (words.next() == Some(id)).then(|| {
                (
                    words.next().unwrap_or_default().to_owned(),
                    words.next().unwrap_or_default().to_owned(),
                )
            })
        });
        if let Some(logged) = logged {
            return logged;
        }
        assert!(
            Instant::now() < deadline,
            "{id}'s agent logged no TMPDIR: {log}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn two_sessions_start_with_temp_folders_of_their_own() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let first = launched_session(&home, &client, "first").await;
    let second = launched_session(&home, &client, "second").await;

    for id in [&first, &second] {
        let expected = home.session_temp_dir(id).display().to_string();
        assert_eq!(
            logged_temp_variables(&home, id).await,
            (expected.clone(), expected)
        );
        let mode = std::fs::metadata(home.session_temp_dir(id))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "{id}'s temp folder is not owner-only");
    }
    assert_ne!(
        home.session_temp_dir(&first),
        home.session_temp_dir(&second)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_session_removes_its_temp_folder() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let id = launched_session(&home, &client, "app").await;
    std::fs::write(
        home.session_temp_dir(&id).join("build.log"),
        "kept until the end",
    )
    .unwrap();

    client
        .request(
            Command::SessionEnd {
                session: id.clone(),
            },
            GENEROUS,
        )
        .await
        .unwrap();
    wait_for_state(&client, &id, |state| *state == SessionStateView::Ended).await;

    assert!(!home.session_temp_dir(&id).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn an_asleep_session_keeps_its_temp_folder() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let id = launched_session(&home, &client, "app").await;
    let kept = home.session_temp_dir(&id).join("build.log");
    std::fs::write(&kept, "kept while it can wake").unwrap();

    client
        .request(
            Command::SessionSleep {
                session: id.clone(),
            },
            GENEROUS,
        )
        .await
        .unwrap();
    wait_for_state(&client, &id, |state| *state == SessionStateView::Asleep).await;

    assert_eq!(
        std::fs::read_to_string(&kept).unwrap(),
        "kept while it can wake"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_storage_report_lists_each_sessions_temp_folder_size() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let id = launched_session(&home, &client, "app").await;
    std::fs::write(home.session_temp_dir(&id).join("build.log"), [0; 1234]).unwrap();

    let reply = client
        .request(Command::StorageReport, GENEROUS)
        .await
        .unwrap();

    let Reply::Storage {
        session_temps,
        session_temps_truncated,
        ..
    } = reply
    else {
        panic!("storage-report answered {reply:?}");
    };
    assert!(!session_temps_truncated);
    let listed = session_temps
        .as_deref()
        .unwrap_or_default()
        .iter()
        .find(|temp| temp.session == id)
        .unwrap_or_else(|| panic!("{id} missing from {session_temps:?}"));
    assert_eq!(
        Path::new(&listed.path),
        home.session_temp_dir(&id).as_path()
    );
    assert_eq!(
        listed.measurement,
        TargetSizeView::Complete {
            apparent_bytes: 1234
        }
    );
}

/// The agent writes its folder and may widen its mode; the next launch makes it owner-only
/// again, so no other user can plant files there for the woken agent.
#[tokio::test(flavor = "multi_thread")]
async fn waking_makes_a_widened_temp_folder_owner_only_again() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let id = launched_session(&home, &client, "app").await;
    client
        .request(
            Command::SessionSleep {
                session: id.clone(),
            },
            GENEROUS,
        )
        .await
        .unwrap();
    wait_for_state(&client, &id, |state| *state == SessionStateView::Asleep).await;
    let folder = home.session_temp_dir(&id);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o777)).unwrap();

    client
        .request(
            Command::SessionWake {
                session: id.clone(),
            },
            GENEROUS,
        )
        .await
        .unwrap();
    home.wait_for_launches(&id, 2).await;

    let mode = std::fs::metadata(&folder).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}
