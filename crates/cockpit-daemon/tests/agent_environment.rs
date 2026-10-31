//! An agent the daemon launches reaches the daemon's agent socket whatever XDG layout the
//! daemon runs with (#268): the real daemon binary on scratch `XDG_*` directories launches a
//! stand-in Claude Code whose `cockpit agent report` must land.

mod real_daemon;

use std::time::{Duration, Instant};

use cockpit_protocol::{Command, Reply};
use real_daemon::{GENEROUS, Home, REPORT_PROJECT, REPORTED_LABEL, cockpit_program, new_session};

#[tokio::test(flavor = "multi_thread")]
async fn a_launched_agent_reports_to_a_daemon_on_a_scratch_layout() {
    let cockpit = cockpit_program();
    assert!(
        cockpit.is_file(),
        "{} is missing: run the workspace's tests (`make crates-test`), which build it",
        cockpit.display()
    );
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;
    let (root, _) = home.project(&client, REPORT_PROJECT).await;
    let session = new_session(&client, &root).await;

    let deadline = Instant::now() + GENEROUS;
    loop {
        let reports = match client.request(Command::AgentReports, GENEROUS).await {
            Ok(Reply::AgentReports { reports }) => reports,
            other => panic!("agent-reports answered {other:?}"),
        };
        let landed = reports.iter().any(|report| {
            report.session == session.id && report.label.as_deref() == Some(REPORTED_LABEL)
        });
        if landed {
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
