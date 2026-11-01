//! Issue #354 through the real daemon binary and the real `cockpit`: the owner's session
//! settings changed on disk reach the running daemon at `cockpit config reload`, which
//! prints what it applied; an invalid file is refused and the settings in force stay.

mod real_daemon;

use cockpit_client::{ClientError, Connection};
use cockpit_protocol::{Command, ErrorCode};
use real_daemon::{GENEROUS, Home, cockpit_program};

/// A daemon started inside a graphical session.
const DESKTOP_AT_START: [(&str, &str); 2] = [("DISPLAY", ":7"), ("WAYLAND_DISPLAY", "wayland-7")];

const WITHHELD: &str = "[agents]\ndesktop_session = \"withheld\"\n";

/// Runs `cockpit config reload` as the owner and returns its output.
async fn reload(home: &Home) -> std::process::Output {
    assert!(
        cockpit_program().is_file(),
        "{} is missing: run the workspace's tests (`make crates-test`), which build it",
        cockpit_program().display()
    );
    home.cockpit(&["config", "reload"], &[]).await
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reload_prints_each_session_setting_and_the_value_it_replaced() {
    let home = Home::new();
    let _daemon = home.start();
    home.write_owner_config(
        "[agents]\ndesktop_session = \"withheld\"\n\
         [sessions]\nidle_sleep_after_minutes = 45\n",
    );

    let reloaded = reload(&home).await;

    assert!(reloaded.status.success(), "{}", text(&reloaded.stderr));
    assert_eq!(
        text(&reloaded.stdout),
        "sessions.idle_sleep_after_minutes = 45  # was 60\n\
         sessions.asleep_expiry_days = 7\n\
         agents.desktop_session = \"withheld\"  # was \"shared\"\n\
         agents.sandbox = \"required\"\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_launched_after_a_reload_to_withheld_gets_no_desktop_session() {
    let home = Home::new();
    let _daemon = home.start_with(&DESKTOP_AT_START);
    home.write_owner_config(WITHHELD);

    let reloaded = reload(&home).await;
    let seen = home.environment_of_a_launched_agent().await;

    assert!(reloaded.status.success(), "{}", text(&reloaded.stderr));
    assert_eq!(
        seen,
        "DISPLAY=unset\nXAUTHORITY=unset\nWAYLAND_DISPLAY=unset\nDBUS_SESSION_BUS_ADDRESS=unset\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_import_after_a_reload_to_withheld_says_agents_are_kept_off_it() {
    let home = Home::new();
    let _daemon = home.start();
    home.write_owner_config(WITHHELD);
    let reloaded = reload(&home).await;
    assert!(reloaded.status.success(), "{}", text(&reloaded.stderr));

    let imported = home
        .cockpit(&["desktop-session", "import"], &[("DISPLAY", ":8")])
        .await;

    assert!(imported.status.success(), "{}", text(&imported.stderr));
    assert!(
        text(&imported.stderr).contains("withheld"),
        "{}",
        text(&imported.stderr)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_owner_file_is_refused_and_the_settings_in_force_stay() {
    let home = Home::new();
    home.write_owner_config(WITHHELD);
    let _daemon = home.start();
    home.write_owner_config("[agents]\ndesktop_session = \"sometimes\"\n");

    let refused = reload(&home).await;
    home.write_owner_config(WITHHELD);
    let unchanged = reload(&home).await;

    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused.stdout));
    assert!(
        text(&refused.stderr).contains("invalid-argument: the owner configuration is invalid"),
        "{}",
        text(&refused.stderr)
    );
    assert_eq!(
        text(&unchanged.stdout),
        "sessions.idle_sleep_after_minutes = 60\n\
         sessions.asleep_expiry_days = 7\n\
         agents.desktop_session = \"withheld\"\n\
         agents.sandbox = \"required\"\n"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_may_not_reload_the_owner_configuration() {
    let home = Home::new();
    let _daemon = home.start();
    let agent = Connection::open(&home.agent_socket_path(), GENEROUS)
        .await
        .unwrap();

    let refused = agent.request(Command::ConfigReload, GENEROUS).await;

    assert!(
        matches!(&refused, Err(ClientError::Daemon(error)) if error.code == ErrorCode::NotAuthorized),
        "{refused:?}"
    );
}
