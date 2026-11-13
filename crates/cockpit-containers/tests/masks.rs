//! The daemon asks Torlek's masks helper (#390, `torlek-sandbox engine-masks`) to show a
//! project's engine one checkout masked before a session reaches that engine. The fake helper
//! listens as this test's own user, so that user stands for root.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cockpit_containers::{EngineUser, MasksHelper};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

const DEADLINE: Duration = Duration::from_secs(5);

struct FakeHelper {
    _dir: tempfile::TempDir,
    socket: PathBuf,
    requests: Arc<Mutex<Vec<String>>>,
}

/// Reads each connection's line, then answers `answer`, as the helper does; `None` never
/// answers.
fn fake_helper(answer: Option<&'static str>) -> FakeHelper {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("engine-masks.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0u8; 512];
                while !request.ends_with(b"\n") {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => request.extend_from_slice(&chunk[..read]),
                    }
                }
                seen.lock()
                    .unwrap()
                    .push(String::from_utf8(request).unwrap());
                match answer {
                    Some(answer) => {
                        let _ = stream.write_all(answer.as_bytes()).await;
                    }
                    None => tokio::time::sleep(Duration::from_secs(60)).await,
                }
            });
        }
    });
    FakeHelper {
        _dir: dir,
        socket,
        requests,
    }
}

fn own_uid(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().uid()
}

fn helper_at(fake: &FakeHelper, uid: u32) -> MasksHelper {
    MasksHelper {
        socket: fake.socket.clone(),
        uid,
    }
}

fn engine_user() -> EngineUser {
    EngineUser::for_project("torlek", 3).unwrap()
}

#[tokio::test]
async fn a_checkout_the_helper_masks_is_shown_after_naming_its_engine() {
    let fake = fake_helper(Some("ok\n"));
    let helper = helper_at(&fake, own_uid(&fake.socket));

    let shown = helper
        .show_masked(&engine_user(), Path::new("/home/owner/shop"), DEADLINE)
        .await;

    assert_eq!(shown, Ok(()));
    assert_eq!(
        *fake.requests.lock().unwrap(),
        ["torlek-e3 /home/owner/shop\n"]
    );
}

#[tokio::test]
async fn a_refusal_names_the_helpers_reason() {
    let fake = fake_helper(Some("refused: /home/owner/shop holds no .git\n"));
    let helper = helper_at(&fake, own_uid(&fake.socket));

    let refused = helper
        .show_masked(&engine_user(), Path::new("/home/owner/shop"), DEADLINE)
        .await
        .unwrap_err();

    assert!(
        refused.contains("/home/owner/shop holds no .git"),
        "{refused}"
    );
}

#[tokio::test]
async fn a_listener_other_than_the_helpers_user_is_told_nothing() {
    let fake = fake_helper(Some("ok\n"));
    let helper = helper_at(&fake, own_uid(&fake.socket) + 1);

    let refused = helper
        .show_masked(&engine_user(), Path::new("/home/owner/shop"), DEADLINE)
        .await
        .unwrap_err();

    assert!(refused.contains("not Torlek's masks helper"), "{refused}");
    assert!(fake.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_helper_that_does_not_answer_in_time_masks_nothing() {
    let fake = fake_helper(None);
    let helper = helper_at(&fake, own_uid(&fake.socket));

    let refused = helper
        .show_masked(
            &engine_user(),
            Path::new("/home/owner/shop"),
            Duration::from_millis(200),
        )
        .await
        .unwrap_err();

    assert!(refused.contains("no answer within"), "{refused}");
}

#[tokio::test]
async fn an_answer_that_is_neither_ok_nor_a_refusal_masks_nothing() {
    let fake = fake_helper(Some("okay\n"));
    let helper = helper_at(&fake, own_uid(&fake.socket));

    let refused = helper
        .show_masked(&engine_user(), Path::new("/home/owner/shop"), DEADLINE)
        .await
        .unwrap_err();

    assert!(refused.contains("\"okay\""), "{refused}");
}

#[tokio::test]
async fn a_checkout_whose_path_holds_a_newline_is_never_asked_for() {
    let fake = fake_helper(Some("ok\n"));
    let helper = helper_at(&fake, own_uid(&fake.socket));

    let refused = helper
        .show_masked(&engine_user(), Path::new("/home/owner/shop\nx"), DEADLINE)
        .await
        .unwrap_err();

    assert!(refused.contains("newline"), "{refused}");
    assert!(fake.requests.lock().unwrap().is_empty());
}

#[test]
fn the_installed_helper_listens_where_the_units_put_it_as_root() {
    let helper = MasksHelper::installed();

    assert_eq!(helper.socket, Path::new("/run/torlek-engine-masks.sock"));
    assert_eq!(helper.uid, 0);
}

#[tokio::test]
async fn a_helper_socket_the_owner_reaches_is_ready() {
    let fake = fake_helper(Some("ok\n"));
    let helper = helper_at(&fake, 0);

    assert_eq!(helper.check_socket(own_uid(&fake.socket)), Ok(()));
}

#[tokio::test]
async fn a_helper_socket_that_is_missing_another_users_or_no_socket_is_not_ready() {
    let fake = fake_helper(Some("ok\n"));
    let owner = own_uid(&fake.socket);
    let plain_file = fake._dir.path().join("plain");
    std::fs::write(&plain_file, "").unwrap();
    for (case, socket, owner_uid, expected) in [
        (
            "missing",
            fake._dir.path().join("absent.sock"),
            owner,
            "is not installed",
        ),
        ("not a socket", plain_file, owner, "is not a socket"),
        ("another user's", fake.socket.clone(), owner + 1, "only uid"),
    ] {
        let helper = MasksHelper { socket, uid: 0 };

        let refused = helper.check_socket(owner_uid).unwrap_err();

        assert!(refused.contains(expected), "{case}: {refused}");
    }
}
