//! What an agent the daemon launches inherits from the daemon, through the real daemon
//! binary and a stand-in Claude Code. It reaches the daemon's agent socket whatever XDG
//! layout the daemon runs with (#268), and the owner's desktop session through the display
//! variables the daemon has, never ones it lacks (#303).

mod real_daemon;

use std::time::{Duration, Instant};

use cockpit_protocol::{Command, Reply};
use real_daemon::{
    ENVIRONMENT_PROJECT, GENEROUS, Home, REPORT_PROJECT, REPORTED_LABEL, cockpit_program,
    new_session,
};

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

#[tokio::test(flavor = "multi_thread")]
async fn a_launched_agent_sees_the_daemons_display_variables() {
    let home = Home::new();
    let _daemon = home.start_with(&[
        ("DISPLAY", ":7"),
        ("XAUTHORITY", "/run/user/7/gdm/Xauthority"),
        ("WAYLAND_DISPLAY", "wayland-7"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/7/bus"),
    ]);

    let seen = environment_of_a_launched_agent(&home).await;

    assert_eq!(
        seen,
        "DISPLAY=:7\nXAUTHORITY=/run/user/7/gdm/Xauthority\nWAYLAND_DISPLAY=wayland-7\n\
         DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/7/bus\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launched_agent_gets_no_display_variable_the_daemon_lacks() {
    let home = Home::new();
    let _daemon = home.start_with(&[("WAYLAND_DISPLAY", "wayland-7")]);

    let seen = environment_of_a_launched_agent(&home).await;

    assert_eq!(
        seen,
        "DISPLAY=unset\nXAUTHORITY=unset\nWAYLAND_DISPLAY=wayland-7\nDBUS_SESSION_BUS_ADDRESS=unset\n"
    );
}

/// Starts a session of the [`ENVIRONMENT_PROJECT`] agent and returns what it wrote.
async fn environment_of_a_launched_agent(home: &Home) -> String {
    let client = home.connect().await;
    let (root, _) = home.project(&client, ENVIRONMENT_PROJECT).await;
    new_session(&client, &root).await;
    let deadline = Instant::now() + GENEROUS;
    loop {
        if let Ok(written) = std::fs::read_to_string(home.environment_log()) {
            return written;
        }
        assert!(Instant::now() < deadline, "the agent wrote no environment");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
