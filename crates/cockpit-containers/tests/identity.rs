//! A project's engine is verified through its socket (#390): the kernel names the listening
//! user, the engine's `/info` says rootless and names its ID, and the owner's record decides
//! which ID is the project's. The fake engine listens as this test's own user, so that user
//! stands for the engine user and a made-up id for the owner.

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cockpit_containers::{
    EngineState, EngineUser, Expected, RecordedEngine, Refusal, Remedy, ask_socket, verify,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixListener;

const ROOTLESS_INFO: &str = r#"{"ID":"engine-7c2b","Name":"box","ServerVersion":"28.1.1","SecurityOptions":["name=seccomp,profile=builtin","name=rootless","name=cgroupns"]}"#;
const ROOTFUL_INFO: &str = r#"{"ID":"engine-root","Name":"box","ServerVersion":"28.1.1","SecurityOptions":["name=seccomp,profile=builtin"]}"#;
/// Someone else: the owner, in these tests.
const OWNER_UID: u32 = 4_000_000;

struct FakeEngine {
    _dir: tempfile::TempDir,
    socket: PathBuf,
}

/// Answers every connection with `answer` once the request ends, as `dockerd` does.
fn fake_engine(answer: String) -> FakeEngine {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("docker.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let answer = answer.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0u8; 512];
                while !request.ends_with(b"\r\n\r\n") {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(read) => request.extend_from_slice(&chunk[..read]),
                    }
                }
                assert!(request.starts_with(b"GET /info HTTP/1.0\r\n"));
                let _ = stream.write_all(answer.as_bytes()).await;
            });
        }
    });
    FakeEngine { _dir: dir, socket }
}

fn ok(body: &str) -> String {
    format!(
        "HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn own_uid(socket: &Path) -> u32 {
    std::fs::metadata(socket).unwrap().uid()
}

fn user() -> EngineUser {
    EngineUser::for_project("torlek", 3).unwrap()
}

fn recorded(engine_id: &str) -> RecordedEngine {
    RecordedEngine {
        project: "shop".to_owned(),
        user: "torlek-e3".to_owned(),
        engine_id: engine_id.to_owned(),
        recorded_at: 1_000,
        recorded_through: Some("cli".to_owned()),
    }
}

async fn state_at(socket: &Path, user_uid: u32, record: Option<&RecordedEngine>) -> EngineState {
    let user = user();
    let answer = ask_socket(socket, Duration::from_secs(5)).await;
    verify(
        answer,
        Expected {
            user: &user,
            user_uid: Some(user_uid),
            owner_uid: OWNER_UID,
        },
        record,
    )
}

#[tokio::test]
async fn the_projects_rootless_engine_is_unrecorded_until_the_owner_records_it() {
    let engine = fake_engine(ok(ROOTLESS_INFO));
    let uid = own_uid(&engine.socket);

    let state = state_at(&engine.socket, uid, None).await;

    let EngineState::Unrecorded(info) = state else {
        panic!("{state:?}")
    };
    assert_eq!(info.id, "engine-7c2b");
    assert!(info.rootless);
}

#[tokio::test]
async fn the_recorded_engine_is_verified() {
    let engine = fake_engine(ok(ROOTLESS_INFO));
    let uid = own_uid(&engine.socket);

    let state = state_at(&engine.socket, uid, Some(&recorded("engine-7c2b"))).await;

    assert!(
        matches!(state, EngineState::Verified(ref info) if info.id == "engine-7c2b"),
        "{state:?}"
    );
}

#[tokio::test]
async fn an_engine_other_than_the_recorded_one_is_refused_until_recorded_again() {
    let engine = fake_engine(ok(ROOTLESS_INFO));
    let uid = own_uid(&engine.socket);

    let state = state_at(&engine.socket, uid, Some(&recorded("engine-before"))).await;

    let EngineState::Refused(refusal) = state else {
        panic!("{state:?}")
    };
    assert_eq!(
        refusal,
        Refusal::NotRecorded {
            found: "engine-7c2b".to_owned(),
            recorded: "engine-before".to_owned()
        }
    );
    assert_eq!(refusal.remedy(), Remedy::Record);
}

#[tokio::test]
async fn an_engine_run_by_another_user_than_the_projects_is_refused() {
    let engine = fake_engine(ok(ROOTLESS_INFO));
    let uid = own_uid(&engine.socket);

    let state = state_at(&engine.socket, uid + 1, None).await;

    assert!(
        matches!(state, EngineState::Refused(Refusal::OtherUser { uid: found, .. }) if found == uid),
        "{state:?}"
    );
}

#[tokio::test]
async fn the_owners_own_engine_is_refused() {
    let engine = fake_engine(ok(ROOTLESS_INFO));
    let uid = own_uid(&engine.socket);
    let user = user();

    let state = verify(
        ask_socket(&engine.socket, Duration::from_secs(5)).await,
        Expected {
            user: &user,
            user_uid: Some(uid),
            owner_uid: uid,
        },
        None,
    );

    assert_eq!(state, EngineState::Refused(Refusal::Owner { uid }));
}

#[tokio::test]
async fn an_engine_that_does_not_say_rootless_is_refused() {
    let engine = fake_engine(ok(ROOTFUL_INFO));
    let uid = own_uid(&engine.socket);

    let state = state_at(&engine.socket, uid, None).await;

    assert_eq!(
        state,
        EngineState::Refused(Refusal::Rootful {
            engine_id: "engine-root".to_owned()
        })
    );
}

#[tokio::test]
async fn no_engine_and_no_record_is_the_setup_step() {
    let dir = tempfile::tempdir().unwrap();

    let state = state_at(&dir.path().join("docker.sock"), 961, None).await;

    assert!(
        matches!(state, EngineState::NotSetUp { ref reason } if reason.contains("cannot connect")),
        "{state:?}"
    );
}

#[tokio::test]
async fn a_recorded_engine_that_does_not_answer_is_to_be_started() {
    let dir = tempfile::tempdir().unwrap();

    let state = state_at(
        &dir.path().join("docker.sock"),
        961,
        Some(&recorded("engine-7c2b")),
    )
    .await;

    let EngineState::Refused(refusal) = state else {
        panic!("{state:?}")
    };
    assert!(matches!(refusal, Refusal::Unreachable { .. }));
    assert_eq!(refusal.remedy(), Remedy::Start);
}

#[tokio::test]
async fn an_engine_user_that_is_gone_is_the_setup_step_or_a_refusal() {
    let user = user();
    let expected = Expected {
        user: &user,
        user_uid: None,
        owner_uid: OWNER_UID,
    };

    let unrecorded = verify(Err("unused".to_owned()), expected, None);
    let recorded = verify(
        Err("unused".to_owned()),
        expected,
        Some(&recorded("engine-7c2b")),
    );

    assert!(
        matches!(unrecorded, EngineState::NotSetUp { ref reason } if reason.contains("no user torlek-e3")),
        "{unrecorded:?}"
    );
    assert_eq!(
        recorded,
        EngineState::Refused(Refusal::NoUser {
            user: "torlek-e3".to_owned()
        })
    );
}

#[tokio::test]
async fn an_answer_that_is_not_info_names_its_status() {
    let engine = fake_engine("HTTP/1.0 404 Not Found\r\n\r\npage not found".to_owned());

    let error = ask_socket(&engine.socket, Duration::from_secs(5))
        .await
        .unwrap_err();

    assert!(error.contains("404 Not Found"), "{error}");
}

#[tokio::test]
async fn an_engine_that_never_answers_is_unreachable_within_the_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("docker.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let held = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(stream);
    });
    let started = Instant::now();

    let error = ask_socket(&socket, Duration::from_millis(300))
        .await
        .unwrap_err();

    assert!(error.contains("no answer within"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
    held.abort();
}
