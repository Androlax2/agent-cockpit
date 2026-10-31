mod common;

use std::time::{Duration, Instant};

use cockpit_client::{ClientError, CloseReason, Connection};
use cockpit_protocol::{
    HandshakeReply, Hello, ProtocolVersion, Refusal, RefusalReason, SUPPORTED_VERSIONS, Welcome,
    negotiate,
};
use common::{FakeDaemon, GENEROUS};
use serde_json::json;

/// The client's side of a handshake against a daemon that answers its hello with `reply`.
async fn open_against(reply: &HandshakeReply) -> Result<Connection, ClientError> {
    let daemon = FakeDaemon::start();
    let (result, _) = tokio::join!(
        Connection::open(&daemon.socket_path, GENEROUS),
        daemon.answer_hello(reply)
    );
    result
}

/// The change feed is negotiated only by a Subscription, so a plain connection's
/// `subscribe` meets the daemon's `not-negotiated`.
#[tokio::test]
async fn a_plain_connection_offers_every_version_and_asks_for_no_capability() {
    let daemon = FakeDaemon::start();
    let fake = async {
        let mut peer = daemon.accept().await;
        let hello = peer.read_frame().await;
        let parsed: Hello = serde_json::from_value(hello.clone()).unwrap();
        peer.send(&negotiate(&parsed, &SUPPORTED_VERSIONS, &[]))
            .await;
        (hello, peer)
    };

    let (connection, (hello, _peer)) =
        tokio::join!(Connection::open(&daemon.socket_path, GENEROUS), fake);

    assert_eq!(
        hello,
        json!({"protocol_versions": SUPPORTED_VERSIONS, "capabilities": []})
    );
    assert_eq!(
        connection.unwrap().welcome(),
        &Welcome {
            protocol_version: SUPPORTED_VERSIONS[0],
            capabilities: vec![],
        }
    );
}

#[tokio::test]
async fn a_refused_handshake_reports_the_daemons_reason() {
    let refusal = Refusal {
        reason: RefusalReason::VersionMismatch,
        message: "no shared protocol version".to_owned(),
        supported_versions: vec![ProtocolVersion(7)],
    };

    let result = open_against(&HandshakeReply::Refused(refusal.clone())).await;

    match result {
        Err(ClientError::Refused(received)) => assert_eq!(received, refusal),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_daemon_choosing_a_version_the_client_did_not_offer_is_refused() {
    let result = open_against(&HandshakeReply::Accepted(Welcome {
        protocol_version: ProtocolVersion(999),
        capabilities: vec![],
    }))
    .await;

    assert!(
        matches!(
            result,
            Err(ClientError::Closed(CloseReason::InvalidFrame(_)))
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_silent_daemon_fails_the_handshake_at_its_deadline() {
    let daemon = FakeDaemon::start();
    let deadline = Duration::from_millis(200);
    let started = Instant::now();
    let fake = async {
        let mut peer = daemon.accept().await;
        peer.read_frame().await;
        peer
    };

    let (result, _peer) = tokio::join!(Connection::open(&daemon.socket_path, deadline), fake);

    assert!(
        matches!(result, Err(ClientError::DeadlineExceeded)),
        "{result:?}"
    );
    assert!((deadline..GENEROUS).contains(&started.elapsed()));
}

#[tokio::test]
async fn a_daemon_closing_before_its_welcome_is_a_disconnect() {
    let daemon = FakeDaemon::start();
    let fake = async {
        let mut peer = daemon.accept().await;
        peer.read_frame().await;
    };

    let (result, ()) = tokio::join!(Connection::open(&daemon.socket_path, GENEROUS), fake);

    assert!(
        matches!(result, Err(ClientError::Closed(CloseReason::Disconnected))),
        "{result:?}"
    );
}

#[tokio::test]
async fn a_missing_socket_names_its_path() {
    let missing = std::env::temp_dir().join("cockpit-client-no-daemon-here.sock");

    let result = Connection::open(&missing, GENEROUS).await;

    match result {
        Err(ClientError::Connect { socket_path, .. }) => assert_eq!(socket_path, missing),
        other => panic!("expected a connect error, got {other:?}"),
    }
}
