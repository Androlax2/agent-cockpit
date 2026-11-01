//! What an agent the daemon launches inherits from the daemon, through the real daemon
//! binary and a stand-in Claude Code. It reaches the daemon's agent socket whatever XDG
//! layout the daemon runs with (#268), and the owner's desktop session through the display
//! variables the daemon has, never ones it lacks (#303, #306), unless the owner withholds them
//! (#305), and as the owner last imported them (#307).

mod real_daemon;

use std::time::{Duration, Instant};

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, ErrorCode, Reply};
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
    let _daemon = home.start_with(&DESKTOP_AT_START);

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

/// All four desktop variables, as a daemon started inside a graphical session has them.
const DESKTOP_AT_START: [(&str, &str); 4] = [
    ("DISPLAY", ":7"),
    ("XAUTHORITY", "/run/user/7/gdm/Xauthority"),
    ("WAYLAND_DISPLAY", "wayland-7"),
    ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/7/bus"),
];

#[tokio::test(flavor = "multi_thread")]
async fn an_owner_who_withholds_the_desktop_session_launches_agents_without_any_of_it() {
    let home = Home::new();
    home.write_owner_config("[agents]\ndesktop_session = \"withheld\"\n");
    let _daemon = home.start_with(&DESKTOP_AT_START);

    let seen = environment_of_a_launched_agent(&home).await;

    assert_eq!(
        seen,
        "DISPLAY=unset\nXAUTHORITY=unset\nWAYLAND_DISPLAY=unset\nDBUS_SESSION_BUS_ADDRESS=unset\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_launched_after_an_import_sees_the_imported_desktop_session() {
    let home = Home::new();
    let _daemon = home.start_with(&DESKTOP_AT_START);

    let imported = home
        .cockpit(
            &["desktop-session", "import"],
            &[("DISPLAY", ":8"), ("WAYLAND_DISPLAY", "wayland-8")],
        )
        .await;
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let seen = environment_of_a_launched_agent(&home).await;

    assert_eq!(
        seen,
        "DISPLAY=:8\nXAUTHORITY=unset\nWAYLAND_DISPLAY=wayland-8\nDBUS_SESSION_BUS_ADDRESS=unset\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_not_import_a_desktop_session() {
    let home = Home::new();
    let _daemon = home.start();
    let agent = Connection::open(&home.agent_socket_path(), GENEROUS)
        .await
        .unwrap();

    let refused = agent.request(display_import("DISPLAY"), GENEROUS).await;

    assert!(
        matches!(&refused, Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized),
        "{refused:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_of_another_variable_or_a_nul_byte_is_refused_with_its_name() {
    let home = Home::new();
    let _daemon = home.start();
    let client = home.connect().await;

    for (name, value) in [("LD_PRELOAD", ":9"), ("DISPLAY", ":9\0x")] {
        let import = Command::DesktopSessionImport {
            variables: [(name.to_owned(), value.to_owned())].into(),
        };
        let refused = client.request(import, GENEROUS).await;

        assert!(
            matches!(&refused, Err(ClientError::Daemon(error))
                if error.code == ErrorCode::InvalidArgument && error.message.contains(name)),
            "{name}: {refused:?}"
        );
    }
}

/// An import that sets `name` alone.
fn display_import(name: &str) -> Command {
    Command::DesktopSessionImport {
        variables: [(name.to_owned(), ":9".to_owned())].into(),
    }
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
