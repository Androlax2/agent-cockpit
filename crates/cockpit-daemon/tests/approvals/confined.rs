//! #1387: an approved run is confined in a systemd user service of its own. A test home
//! reaches no systemd user manager, so there a confined run is refused, never run
//! unconfined; the tests marked ignored give the daemon this user's real manager, which
//! enforces the limits on a fixture home.
use std::path::PathBuf;

use super::*;

/// The daemon of `home` with confined runs required, as the product default has them.
async fn confining(home: TestHome) -> Asking {
    std::fs::create_dir_all(home.config_file().parent().unwrap()).unwrap();
    std::fs::write(
        home.config_file(),
        "[agents]\nconfined_runs = \"required\"\n",
    )
    .unwrap();
    started_in(home, true).await
}

/// A home whose daemon reaches this user's systemd manager: its runtime folder links the
/// manager's sockets, which `systemctl --user` finds there.
fn with_the_real_manager() -> TestHome {
    let home = TestHome::new();
    let runtime = home.runtime_base.as_ref().unwrap();
    let user_runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap());
    for socket in ["bus", "systemd"] {
        std::os::unix::fs::symlink(user_runtime.join(socket), runtime.join(socket)).unwrap();
    }
    home
}

#[tokio::test]
async fn a_confined_run_without_a_systemd_manager_is_refused_never_run_unconfined() {
    let asking = confining(TestHome::new()).await;
    let marker = asking.cwd.path().join("ran");

    let ended = asking.ran(&format!("touch '{}'", marker.display())).await;

    assert_eq!(ended.state, RunStateView::NotRun, "{ended:?}");
    let note = ended.note.unwrap_or_default();
    assert!(note.contains("confined"), "{note}");
    assert!(!marker.exists(), "the command ran unconfined");
}

#[tokio::test]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_confined_run_cannot_read_the_owners_keys() {
    let asking = confining(with_the_real_manager()).await;
    let keys = asking.home.home().join(".ssh");
    std::fs::create_dir(&keys).unwrap();
    std::fs::write(keys.join("id_test"), "the owner's key").unwrap();

    let ended = asking
        .ran(&format!("cat '{}'", keys.join("id_test").display()))
        .await;

    assert_eq!(ended.state, RunStateView::Finished, "{ended:?}");
    let result = ended.result.unwrap();
    assert_ne!(result.exit_code, 0, "{result:?}");
    assert!(result.output.contains("Permission denied"), "{result:?}");
    assert!(!result.output.contains("the owner's key"), "{result:?}");
}

#[tokio::test]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_confined_run_writes_its_checkout_only() {
    let asking = confining(with_the_real_manager()).await;
    let root = in_a_registered_project(&asking).await;
    let outside = asking.home.home().join("outside");

    let ended = asking
        .ran(&format!(
            "echo in > '{}' && echo out > '{}'",
            root.join("inside").display(),
            outside.display()
        ))
        .await;

    let result = ended.result.unwrap();
    assert_ne!(result.exit_code, 0, "{result:?}");
    assert!(
        result.output.contains("Read-only file system"),
        "{result:?}"
    );
    assert!(root.join("inside").exists(), "{result:?}");
    assert!(!outside.exists(), "{result:?}");
}

/// The run's network namespace has a loopback of its own: a listener on the host's is out
/// of its reach, as every other host is.
#[tokio::test]
#[ignore = "needs a reachable systemd user manager"]
async fn a_real_confined_run_has_no_network() {
    let asking = confining(with_the_real_manager()).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();

    let ended = asking
        .ran(&format!(
            "curl -sS --max-time 5 http://127.0.0.1:{port}/ && echo connected"
        ))
        .await;

    let result = ended.result.unwrap();
    assert_ne!(result.exit_code, 0, "{result:?}");
    assert!(!result.output.contains("connected"), "{result:?}");
}
