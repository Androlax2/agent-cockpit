//! `torlek containers record` against the real daemon binary (#390): the owner records a
//! project's engine only once it answers as the project's own; until then the refusal names
//! the step that sets it up. The engine's socket is the system's (`/run/<user>`), so these
//! tests see the host without one; `cockpit-containers`' tests verify a fake engine.

mod candidate;
mod real_daemon;

use real_daemon::Home;

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn recording_a_project_without_its_engine_is_refused_naming_the_setup_step() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, "app").await;

    let record = home.cockpit(&["containers", "record", "app"], &[]).await;

    let stderr = text(&record.stderr);
    assert!(!record.status.success(), "{stderr}");
    assert!(stderr.contains("no engine to record"), "{stderr}");
    assert!(
        stderr.contains(&format!("`torlek doctor --project {}`", root.display())),
        "{stderr}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recording_an_unregistered_project_is_refused() {
    let home = Home::new();
    let _daemon = home.start();

    let record = home.cockpit(&["containers", "record", "nope"], &[]).await;

    let stderr = text(&record.stderr);
    assert!(!record.status.success(), "{stderr}");
    assert!(stderr.contains("no registered project nope"), "{stderr}");
}
