//! `Provider::deliver` (#407): submit and cancel through the session's app-server, against a
//! fake app-server on the session's socket.

mod common;

use std::path::PathBuf;

use cockpit_codex::Codex;
use cockpit_provider_api::{Action, ConversationId, Delivered, Provider};
use common::app_server::{Answer, FakeAppServer, last_turn};
use serde_json::{Value, json};

const THREAD: &str = "01a1981e-525b-75f2-902e-1e2e6d337eea";
const TURN: &str = "01a1981e-63fd-7022-a2d2-d4a999b8ff29";

fn thread() -> ConversationId {
    ConversationId::new(THREAD).unwrap()
}

/// A session directory with a fake app-server on its socket, answering with `script`.
fn session(
    script: impl Fn(&str, &Value) -> Answer + Send + Sync + 'static,
) -> (tempfile::TempDir, FakeAppServer) {
    let dir = tempfile::tempdir().unwrap();
    let server = FakeAppServer::start(&dir.path().join("codex.sock"), script);
    (dir, server)
}

/// The thread's status as `thread/read` answers it: `active` while a turn is under way.
fn thread_status(turn_status: &str) -> Answer {
    let status = if turn_status == "inProgress" {
        json!({"type": "active", "activeFlags": []})
    } else {
        json!({"type": "idle"})
    };
    Ok(json!({"thread": {"id": THREAD, "status": status}}))
}

/// The last turn is `status` until a turn starts; every other call succeeds.
fn last_turn_is(status: &'static str) -> impl Fn(&str, &Value) -> Answer + Send + Sync {
    move |method, _| match method {
        "thread/read" => thread_status(status),
        "thread/turns/list" => last_turn(TURN, status),
        "turn/start" => Ok(json!({"turn": {"id": "new-turn", "status": "inProgress"}})),
        "turn/steer" => Ok(json!({"turnId": TURN})),
        _ => Ok(json!({})),
    }
}

fn params_of(server: &FakeAppServer, method: &str) -> Value {
    server
        .requests()
        .into_iter()
        .find(|(called, _)| called == method)
        .unwrap_or_else(|| panic!("no {method} in {:?}", server.methods()))
        .1
}

#[test]
fn a_message_to_an_idle_conversation_starts_a_turn_on_its_thread() {
    let (dir, server) = session(last_turn_is("completed"));

    let delivered = common::codex()
        .deliver(&Action::Submit("fix the build"), dir.path(), &thread())
        .unwrap();

    assert_eq!(delivered, Delivered::StartedTurn);
    assert_eq!(
        params_of(&server, "turn/start"),
        json!({"threadId": THREAD, "input": [{"type": "text", "text": "fix the build"}]})
    );
}

#[test]
fn a_message_while_a_turn_is_under_way_joins_that_turn() {
    let (dir, server) = session(last_turn_is("inProgress"));

    let delivered = common::codex()
        .deliver(&Action::Submit("also the docs"), dir.path(), &thread())
        .unwrap();

    assert_eq!(delivered, Delivered::JoinedTurn);
    assert_eq!(
        params_of(&server, "turn/steer"),
        json!({
            "threadId": THREAD,
            "expectedTurnId": TURN,
            "input": [{"type": "text", "text": "also the docs"}]
        })
    );
    assert!(!server.methods().contains(&"turn/start".to_owned()));
}

#[test]
fn a_message_whose_turn_ended_meanwhile_starts_a_new_turn() {
    let reads = std::sync::atomic::AtomicUsize::new(0);
    let (dir, server) = session(move |method, _| match method {
        "thread/read" => {
            let calls = reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            thread_status(if calls == 0 {
                "inProgress"
            } else {
                "completed"
            })
        }
        "thread/turns/list" => last_turn(TURN, "inProgress"),
        "turn/steer" => Err("expected turn does not match the active turn".to_owned()),
        _ => Ok(json!({})),
    });

    let delivered = common::codex()
        .deliver(&Action::Submit("next"), dir.path(), &thread())
        .unwrap();

    assert_eq!(delivered, Delivered::StartedTurn);
    assert_eq!(
        server.methods(),
        [
            "thread/read",
            "thread/turns/list",
            "turn/steer",
            "thread/read",
            "turn/start"
        ]
    );
}

#[test]
fn a_steer_refused_while_its_turn_still_runs_fails_with_codexs_reason() {
    let (dir, server) = session(|method, _| match method {
        "thread/read" => thread_status("inProgress"),
        "thread/turns/list" => last_turn(TURN, "inProgress"),
        "turn/steer" => Err("input is too long".to_owned()),
        _ => Ok(json!({})),
    });

    let refused = common::codex()
        .deliver(&Action::Submit("next"), dir.path(), &thread())
        .unwrap_err();

    assert_eq!(refused.reason, "`turn/steer` failed: input is too long");
    assert!(!server.methods().contains(&"turn/start".to_owned()));
}

#[test]
fn a_refused_message_fails_with_codexs_reason() {
    let (dir, _server) = session(|method, _| match method {
        "thread/read" => thread_status("completed"),
        _ => Err("thread not found".to_owned()),
    });

    let refused = common::codex()
        .deliver(&Action::Submit("hello"), dir.path(), &thread())
        .unwrap_err();

    assert_eq!(refused.reason, "`turn/start` failed: thread not found");
}

/// A new thread's turns cannot be listed before its first message (0.161.0).
#[test]
fn a_first_message_starts_a_turn_without_listing_turns() {
    let (dir, server) = session(|method, _| match method {
        "thread/read" => thread_status("none"),
        "thread/turns/list" => Err("thread is not materialized yet".to_owned()),
        _ => Ok(json!({})),
    });

    let delivered = common::codex()
        .deliver(&Action::Submit("hello"), dir.path(), &thread())
        .unwrap();

    assert_eq!(delivered, Delivered::StartedTurn);
    assert_eq!(server.methods(), ["thread/read", "turn/start"]);
}

#[test]
fn a_cancel_interrupts_the_turn_under_way() {
    let (dir, server) = session(last_turn_is("inProgress"));

    let delivered = common::codex()
        .deliver(&Action::Cancel, dir.path(), &thread())
        .unwrap();

    assert_eq!(delivered, Delivered::Interrupted);
    assert_eq!(
        params_of(&server, "turn/interrupt"),
        json!({"threadId": THREAD, "turnId": TURN})
    );
}

/// Just after a thread's first message, Codex refuses to list its turns for a moment.
#[test]
fn a_turn_listing_refused_for_a_moment_is_retried() {
    let listed = std::sync::atomic::AtomicUsize::new(0);
    let (dir, server) = session(move |method, _| match method {
        "thread/read" => thread_status("inProgress"),
        "thread/turns/list" => {
            if listed.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                Err("list_turns is not supported yet".to_owned())
            } else {
                last_turn(TURN, "inProgress")
            }
        }
        _ => Ok(json!({})),
    });

    let delivered = common::codex()
        .deliver(&Action::Cancel, dir.path(), &thread())
        .unwrap();

    assert_eq!(delivered, Delivered::Interrupted);
    assert_eq!(
        server.methods(),
        [
            "thread/read",
            "thread/turns/list",
            "thread/turns/list",
            "turn/interrupt"
        ]
    );
}

#[test]
fn a_cancel_without_a_turn_under_way_is_refused() {
    let (dir, server) = session(last_turn_is("completed"));

    let refused = common::codex()
        .deliver(&Action::Cancel, dir.path(), &thread())
        .unwrap_err();

    assert_eq!(
        refused.reason,
        format!("no turn is under way in conversation {THREAD}")
    );
    assert!(!server.methods().contains(&"turn/interrupt".to_owned()));
}

#[test]
fn a_session_without_its_app_server_fails_naming_the_socket() {
    let dir = tempfile::tempdir().unwrap();

    let refused = common::codex()
        .deliver(&Action::Submit("hello"), dir.path(), &thread())
        .unwrap_err();

    let socket = dir.path().join("codex.sock");
    assert!(
        refused.reason.starts_with(&format!(
            "cannot reach Codex's app-server at {}",
            socket.display()
        )),
        "{}",
        refused.reason
    );
}

#[test]
fn compact_is_refused_naming_its_capability() {
    let (dir, server) = session(last_turn_is("completed"));

    let refused = common::codex()
        .deliver(&Action::Compact, dir.path(), &thread())
        .unwrap_err();

    assert_eq!(
        refused.reason,
        "it does not report the `compact` capability"
    );
    assert!(server.methods().is_empty());
}

#[test]
fn without_the_bridge_nothing_is_delivered() {
    let (dir, server) = session(last_turn_is("completed"));
    let codex = Codex::new(PathBuf::from("/usr/bin/codex"), None, None);

    let refused = codex
        .deliver(&Action::Submit("hello"), dir.path(), &thread())
        .unwrap_err();

    assert_eq!(refused.reason, "it does not report the `submit` capability");
    assert!(server.methods().is_empty());
}
