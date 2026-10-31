mod common;

use std::time::Duration;

use cockpit_client::{ClientError, CloseReason, Connection, MAX_FRAME_BYTES};
use cockpit_protocol::{Command, ErrorCode, ProtocolError, Reply, ServerFrame};
use common::{FakeDaemon, GENEROUS, Peer};

const SHORT_DEADLINE: Duration = Duration::from_millis(150);

async fn connected(daemon: &FakeDaemon) -> (Connection, Peer) {
    let (connection, peer) = tokio::join!(
        Connection::open(&daemon.socket_path, &[], GENEROUS),
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

#[tokio::test]
async fn a_frame_cut_by_a_disconnect_is_dropped_not_read() {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;
    let fake = async {
        let request = peer.read_request().await;
        let reply = format!(
            r#"{{"type":"response","id":{},"outcome":{{"ok":{{"kind":"po"#,
            request.id.0
        );
        peer.send_raw(reply.as_bytes()).await;
        drop(peer);
    };

    let (result, ()) = tokio::join!(connection.request(Command::Ping, GENEROUS), fake);

    assert!(
        matches!(result, Err(ClientError::Closed(CloseReason::Truncated))),
        "{result:?}"
    );
}

#[tokio::test]
async fn an_oversized_frame_closes_the_connection() {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;
    let fake = async {
        peer.read_request().await;
        peer.send_raw(&vec![b' '; MAX_FRAME_BYTES]).await;
        peer
    };

    let (result, _peer) = tokio::join!(connection.request(Command::Ping, GENEROUS), fake);

    assert!(
        matches!(result, Err(ClientError::Closed(CloseReason::FrameTooLarge))),
        "{result:?}"
    );
}

#[tokio::test]
async fn an_unreadable_frame_closes_the_connection() {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;
    let fake = async {
        peer.read_request().await;
        peer.send_raw(b"{\"type\":\"telepathy\"}\n").await;
        peer
    };

    let (result, _peer) = tokio::join!(connection.request(Command::Ping, GENEROUS), fake);

    assert!(
        matches!(
            result,
            Err(ClientError::Closed(CloseReason::InvalidFrame(_)))
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_frame_the_daemon_could_not_read_closes_the_connection_with_its_error() {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;
    let rejection = ProtocolError {
        code: ErrorCode::InvalidRequest,
        message: "the frame is not JSON".to_owned(),
    };
    let fake = async {
        peer.read_request().await;
        peer.send(&ServerFrame::MalformedFrame(rejection.clone()))
            .await;
        peer
    };

    let (result, _peer) = tokio::join!(connection.request(Command::Ping, GENEROUS), fake);

    match result {
        Err(ClientError::Closed(CloseReason::RejectedFrame(error))) => {
            assert_eq!(error, rejection);
        }
        other => panic!("expected a rejected frame, got {other:?}"),
    }
}

#[tokio::test]
async fn a_change_batch_without_a_subscription_closes_the_connection() {
    let daemon = FakeDaemon::start();
    let (connection, mut peer) = connected(&daemon).await;
    let fake = async {
        peer.read_request().await;
        peer.send_batch(&common::batch(
            cockpit_protocol::FeedPosition::Snapshot,
            common::cursor("e1", 1),
            &[],
        ))
        .await;
        peer
    };

    let (result, _peer) = tokio::join!(connection.request(Command::Ping, GENEROUS), fake);

    assert!(
        matches!(
            result,
            Err(ClientError::Closed(CloseReason::InvalidFrame(_)))
        ),
        "{result:?}"
    );
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
