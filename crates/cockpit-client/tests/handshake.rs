mod common;

use std::time::{Duration, Instant};

use cockpit_client::{ClientError, CloseReason, Connection};
use cockpit_protocol::{
    Capability, HandshakeReply, ProtocolVersion, Refusal, RefusalReason, SUPPORTED_VERSIONS,
    Welcome,
};
use common::{FakeDaemon, GENEROUS};
use serde_json::json;

#[tokio::test]
async fn the_client_offers_its_versions_and_capabilities_and_keeps_the_welcome() {
    let daemon = FakeDaemon::start();
    let client = Connection::open(&daemon.socket_path, &[Capability::ChangeFeed], GENEROUS);
    let fake = async {
        let mut peer = daemon.accept().await;
        let hello = peer.read_frame().await;
        peer.send(&common::handshake_reply_accepting(vec![
            Capability::ChangeFeed,
        ]))
        .await;
        (hello, peer)
    };

    let (connection, (hello, _peer)) = tokio::join!(client, fake);

    assert_eq!(
        hello,
        json!({"protocol_versions": SUPPORTED_VERSIONS, "capabilities": ["change-feed"]})
    );
    assert_eq!(
        connection.unwrap().welcome(),
        &Welcome {
            protocol_version: SUPPORTED_VERSIONS[0],
            capabilities: vec![Capability::ChangeFeed],
        }
    );
}

#[tokio::test]
async fn a_refused_handshake_reports_the_daemons_reason() {
    let daemon = FakeDaemon::start();
    let refusal = Refusal {
        reason: RefusalReason::VersionMismatch,
        message: "no shared protocol version".to_owned(),
        supported_versions: vec![ProtocolVersion(7)],
    };
    let client = Connection::open(&daemon.socket_path, &[], GENEROUS);
    let fake = async {
        let mut peer = daemon.accept().await;
        peer.read_frame().await;
        peer.send(&HandshakeReply::Refused(refusal.clone())).await;
        peer
    };

    let (result, _peer) = tokio::join!(client, fake);

    match result {
        Err(ClientError::Refused(received)) => assert_eq!(received, refusal),
        other => panic!("expected a refusal, got {:?}", other.map(|_| ())),
    }
}

#[tokio::test]
async fn a_daemon_choosing_a_version_the_client_did_not_offer_is_refused() {
    let daemon = FakeDaemon::start();
    let client = Connection::open(&daemon.socket_path, &[], GENEROUS);
    let fake = async {
        let mut peer = daemon.accept().await;
        peer.read_frame().await;
        peer.send(&HandshakeReply::Accepted(Welcome {
            protocol_version: ProtocolVersion(999),
            capabilities: vec![],
        }))
        .await;
        peer
    };

    let (result, _peer) = tokio::join!(client, fake);

    assert!(
        matches!(
            result,
            Err(ClientError::Closed(CloseReason::InvalidFrame(_)))
        ),
        "{:?}",
        result.map(|_| ())
    );
}

#[tokio::test]
async fn a_silent_daemon_fails_the_handshake_at_its_deadline() {
    let daemon = FakeDaemon::start();
    let deadline = Duration::from_millis(200);
    let started = Instant::now();
    let client = Connection::open(&daemon.socket_path, &[], deadline);
    let fake = async {
        let mut peer = daemon.accept().await;
        peer.read_frame().await;
        peer
    };

    let (result, _peer) = tokio::join!(client, fake);

    assert!(
        matches!(result, Err(ClientError::DeadlineExceeded)),
        "{:?}",
        result.map(|_| ())
    );
    assert!((deadline..GENEROUS).contains(&started.elapsed()));
}

#[tokio::test]
async fn a_daemon_closing_before_its_welcome_is_a_disconnect() {
    let daemon = FakeDaemon::start();
    let client = Connection::open(&daemon.socket_path, &[], GENEROUS);
    let fake = async {
        let mut peer = daemon.accept().await;
        peer.read_frame().await;
    };

    let (result, ()) = tokio::join!(client, fake);

    assert!(
        matches!(result, Err(ClientError::Closed(CloseReason::Disconnected))),
        "{:?}",
        result.map(|_| ())
    );
}

#[tokio::test]
async fn a_missing_socket_names_its_path() {
    let missing = std::env::temp_dir().join("cockpit-client-no-daemon-here.sock");

    let result = Connection::open(&missing, &[], GENEROUS).await;

    match result {
        Err(ClientError::Connect { socket_path, .. }) => assert_eq!(socket_path, missing),
        other => panic!("expected a connect error, got {:?}", other.map(|_| ())),
    }
}
