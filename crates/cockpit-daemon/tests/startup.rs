mod common;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixStream;

use cockpit_daemon::Limits;
use cockpit_protocol::{Command, Reply, SUPPORTED_VERSIONS};
use common::{GENEROUS, TestHome};

fn mode(path: &std::path::Path) -> u32 {
    std::fs::metadata(path)
        .expect("the path exists")
        .permissions()
        .mode()
        & 0o777
}

#[tokio::test]
async fn a_daemon_starts_on_a_clean_home_and_answers_ping() {
    let home = TestHome::new();
    let _daemon = home.start();

    let connection = home.connect().await;

    assert_eq!(
        connection.welcome().protocol_version,
        *SUPPORTED_VERSIONS.last().unwrap()
    );
    assert_eq!(
        connection.request(Command::Ping, GENEROUS).await.unwrap(),
        Reply::Pong
    );
}

#[tokio::test]
async fn a_daemon_starts_on_a_clean_home_with_only_home_in_its_environment() {
    let home = TestHome::bare();
    let _daemon = home.start();

    let connection = home.connect().await;

    assert_eq!(
        connection.request(Command::Ping, GENEROUS).await.unwrap(),
        Reply::Pong
    );
}

#[tokio::test]
async fn a_second_daemon_for_the_same_user_refuses_to_start() {
    let home = TestHome::new();
    let _first = home.start();

    let second = home.run_to_exit();

    assert_eq!(second.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("another cockpit daemon of this user is already running")
            && stderr.contains(&home.socket_dir().join("daemon.lock").display().to_string()),
        "{stderr}"
    );
    let still_serving = home.connect().await;
    assert_eq!(
        still_serving
            .request(Command::Ping, GENEROUS)
            .await
            .unwrap(),
        Reply::Pong
    );
}

#[test]
fn the_socket_and_its_directories_are_private_to_the_user() {
    let home = TestHome::new();
    let _daemon = home.start();

    assert_eq!(
        [
            mode(&home.socket_dir()),
            mode(&home.state_home().join("cockpit")),
            mode(&home.socket_path()),
        ],
        [0o700, 0o700, 0o600]
    );
}

#[test]
fn a_socket_directory_others_may_open_stops_startup() {
    let home = TestHome::new();
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(home.socket_dir())
        .unwrap();
    std::fs::set_permissions(home.socket_dir(), std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = home.run_to_exit();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&format!(
            "directory {}: has mode 755; it must be 700",
            home.socket_dir().display()
        )),
        "{stderr}"
    );
    assert_eq!(mode(&home.socket_dir()), 0o755, "never tightened in place");
}

#[tokio::test]
async fn without_xdg_runtime_dir_the_socket_lives_under_the_state_directory() {
    let mut home = TestHome::new();
    home.runtime_base = None;
    let _daemon = home.start();

    let connection = home.connect().await;

    assert!(home.socket_path().starts_with(home.state_home()));
    assert_eq!(
        connection.request(Command::Ping, GENEROUS).await.unwrap(),
        Reply::Pong
    );
}

#[tokio::test]
async fn a_daemon_starts_after_a_killed_one_left_its_socket_behind() {
    let home = TestHome::new();
    home.start().kill();
    assert!(
        home.socket_path().exists(),
        "kill -9 leaves the socket file"
    );

    let _restarted = home.start();

    let connection = home.connect().await;
    assert_eq!(
        connection.request(Command::Ping, GENEROUS).await.unwrap(),
        Reply::Pong
    );
}

#[test]
fn startup_reconciles_durable_state_before_accepting_requests() {
    let home = TestHome::new();
    let _daemon = home.start();

    let log = home.log_once_it_contains("accepting requests");
    let position = |message: &str| {
        log.find(message)
            .unwrap_or_else(|| panic!("{message:?} is not in the log:\n{log}"))
    };
    let opened = position("opened the state database");
    let reconciled = position("reconciled durable state");
    let accepting = position("accepting requests");
    assert!(opened < reconciled && reconciled < accepting, "{log}");
}

fn handshaken(home: &TestHome) -> UnixStream {
    let mut stream = UnixStream::connect(home.socket_path()).expect("the daemon accepts");
    stream
        .write_all(b"{\"protocol_versions\":[1]}\n")
        .expect("the hello is sent");
    let mut welcome = String::new();
    BufReader::new(&stream)
        .read_line(&mut welcome)
        .expect("the daemon answers");
    assert!(welcome.contains("accepted"), "{welcome}");
    stream
}

#[test]
fn cockpit_log_level_decides_which_records_the_daemon_writes() {
    let mut home = TestHome::new();
    home.log_level = Some("warn".to_owned());
    let _daemon = home.start();

    // The binary serves `Limits::default()`. Handshaken connections hold their places with
    // no deadline, so one more is closed with a warning, written after every record
    // startup made.
    let _held: Vec<UnixStream> = (0..Limits::default().connections)
        .map(|_| handshaken(&home))
        .collect();
    let _over_the_limit = UnixStream::connect(home.socket_path()).expect("the daemon accepts");
    let log = home.log_once_it_contains("too many connections");

    assert!(log.contains("too many connections"), "{log}");
    assert!(!log.contains("accepting requests"), "{log}");
}

#[test]
fn an_unreadable_log_level_stops_startup() {
    let mut home = TestHome::new();
    home.log_level = Some("loud".to_owned());

    let output = home.run_to_exit();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("loud"), "{stderr}");
}
