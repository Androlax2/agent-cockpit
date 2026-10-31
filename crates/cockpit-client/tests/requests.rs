mod common;

use std::time::Duration;

use cockpit_client::{ClientError, CloseReason, Connection, MAX_FRAME_BYTES};
use cockpit_protocol::{Command, ErrorCode, FeedPosition, ProtocolError, Reply, ServerFrame};
use common::{FakeDaemon, GENEROUS, Peer};

const SHORT_DEADLINE: Duration = Duration::from_millis(150);

async fn connected(daemon: &FakeDaemon) -> (Connection, Peer) {
    let (connection, peer) = tokio::join!(
        Connection::open(&daemon.socket_path, GENEROUS),
        daemon.accept_handshake(&[])
    );
    (connection.expect("the handshake succeeds"), peer)
}

#[tokio::test]
async fn concurrent_requests_each_get_their_own_reply_in_any_order() {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;
    let fake = async {
        let first = peer.read_request().await;
        let second = peer.read_request().await;
        peer.reply(second.id, Reply::Pong).await;
        peer.fail(first.id, ErrorCode::Busy).await;
        peer
    };

    let (first, second, _peer) = tokio::join!(
        connection.request(Command::Ping, GENEROUS),
        connection.request(Command::Ping, GENEROUS),
        fake
    );

    let replies = [first, second].map(|result| match result {
        Ok(reply) => Ok(reply),
        Err(ClientError::Daemon(error)) => Err(error.code),
        Err(other) => panic!("unexpected {other}"),
    });
    let mut sorted = replies.to_vec();
    sorted.sort_by_key(|reply| reply.is_err());
    assert_eq!(sorted, [Ok(Reply::Pong), Err(ErrorCode::Busy)]);
}

#[tokio::test]
async fn a_daemon_error_reaches_the_caller_with_its_code() {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;
    let fake = async {
        let request = peer.read_request().await;
        peer.fail(request.id, ErrorCode::NotAuthorized).await;
        peer
    };

    let (result, _peer) = tokio::join!(connection.request(Command::Ping, GENEROUS), fake);

    assert!(
        matches!(
            &result,
            Err(ClientError::Daemon(ProtocolError {
                code: ErrorCode::NotAuthorized,
                ..
            }))
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_request_past_its_deadline_fails_and_its_late_reply_reaches_no_one() {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;
    let ignored = peer.read_request();

    let (late, ignored) = tokio::join!(connection.request(Command::Ping, SHORT_DEADLINE), ignored);

    assert!(
        matches!(late, Err(ClientError::DeadlineExceeded)),
        "{late:?}"
    );
    let fake = async {
        let next = peer.read_request().await;
        peer.fail(ignored.id, ErrorCode::Internal).await;
        peer.reply(next.id, Reply::Pong).await;
        peer
    };
    let (next, _peer) = tokio::join!(connection.request(Command::Ping, GENEROUS), fake);
    assert_eq!(next.unwrap(), Reply::Pong);
}

#[tokio::test]
async fn a_dropped_request_leaves_the_connection_usable_and_its_reply_unclaimed() {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;
    let cancelled = {
        let request = connection.request(Command::Ping, GENEROUS);
        tokio::pin!(request);
        tokio::select! {
            _ = &mut request => panic!("nothing answered the request"),
            cancelled = peer.read_request() => cancelled,
        }
    };

    let fake = async {
        let next = peer.read_request().await;
        peer.fail(cancelled.id, ErrorCode::Internal).await;
        peer.reply(next.id, Reply::Pong).await;
        peer
    };
    let (next, _peer) = tokio::join!(connection.request(Command::Ping, GENEROUS), fake);

    assert_eq!(next.unwrap(), Reply::Pong);
}

#[tokio::test]
async fn requests_in_flight_fail_when_the_daemon_disconnects() {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;
    let fake = async {
        peer.read_request().await;
        drop(peer);
    };

    let (result, ()) = tokio::join!(connection.request(Command::Ping, GENEROUS), fake);

    assert!(
        matches!(result, Err(ClientError::Closed(CloseReason::Disconnected))),
        "{result:?}"
    );
    let after = connection.request(Command::Ping, GENEROUS).await;
    assert!(
        matches!(after, Err(ClientError::Closed(CloseReason::Disconnected))),
        "{after:?}"
    );
}

fn line(frame: &ServerFrame) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(frame).unwrap();
    bytes.push(b'\n');
    bytes
}

/// The daemon answers a request with `answer`, then disconnects when `then_disconnects`.
async fn request_answered_with(
    answer: Vec<u8>,
    then_disconnects: bool,
) -> Result<Reply, ClientError> {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;
    let fake = async {
        peer.read_request().await;
        peer.send_raw(&answer).await;
        if then_disconnects {
            drop(peer);
            None
        } else {
            Some(peer)
        }
    };
    let (result, _peer) = tokio::join!(connection.request(Command::Ping, GENEROUS), fake);
    result
}

type IsExpected = fn(&CloseReason) -> bool;

#[tokio::test]
async fn a_frame_the_client_cannot_use_closes_the_connection_with_its_reason() {
    let rejection = ProtocolError {
        code: ErrorCode::InvalidRequest,
        message: "the frame is not JSON".to_owned(),
    };
    let cases: [(&str, Vec<u8>, bool, IsExpected); 5] = [
        (
            "cut by a disconnect",
            br#"{"type":"response","id":1,"outcome":{"ok":{"kind":"po"#.to_vec(),
            true,
            |reason| *reason == CloseReason::Truncated,
        ),
        ("oversized", vec![b' '; MAX_FRAME_BYTES], false, |reason| {
            *reason == CloseReason::FrameTooLarge
        }),
        (
            "unreadable",
            b"{\"type\":\"telepathy\"}\n".to_vec(),
            false,
            |reason| matches!(reason, CloseReason::InvalidFrame(_)),
        ),
        (
            "a batch without a subscription",
            line(&ServerFrame::Changes(common::batch(
                FeedPosition::Snapshot,
                common::cursor("e1", 1),
                &[],
            ))),
            false,
            |reason| matches!(reason, CloseReason::InvalidFrame(_)),
        ),
        (
            "the daemon could not read the client's frame",
            line(&ServerFrame::MalformedFrame(rejection.clone())),
            false,
            |reason| matches!(reason, CloseReason::RejectedFrame(error) if error.message == "the frame is not JSON"),
        ),
    ];

    for (case, answer, then_disconnects, is_expected) in cases {
        let result = request_answered_with(answer, then_disconnects).await;

        assert!(
            matches!(&result, Err(ClientError::Closed(reason)) if is_expected(reason)),
            "{case}: {result:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_command_is_refused_before_it_is_sent() {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;

    let result = connection.request(Command::Unknown, GENEROUS).await;

    assert!(matches!(result, Err(ClientError::Encode(_))), "{result:?}");
    drop(connection);
    peer.expect_closed().await;
}
