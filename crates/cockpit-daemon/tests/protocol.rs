mod common;

use cockpit_client::ClientError;
use cockpit_protocol::{Command, ErrorCode, FeedPosition, MAX_FRAME_BYTES, ProtocolError, Reply};
use common::{GENEROUS, TestHome};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// A raw connection, for frames `cockpit-client` never sends.
struct RawPeer {
    reader: BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: tokio::net::unix::OwnedWriteHalf,
}

impl RawPeer {
    async fn connect(home: &TestHome) -> RawPeer {
        let stream = UnixStream::connect(home.socket_path()).await.unwrap();
        let (read_half, writer) = stream.into_split();
        RawPeer {
            reader: BufReader::new(read_half),
            writer,
        }
    }

    async fn send_line(&mut self, line: &[u8]) {
        self.writer.write_all(line).await.unwrap();
        self.writer.write_all(b"\n").await.unwrap();
    }

    /// The next frame, or `None` once the daemon closed the connection.
    async fn read_frame(&mut self) -> Option<Value> {
        let mut line = String::new();
        let read = tokio::time::timeout(GENEROUS, self.reader.read_line(&mut line))
            .await
            .expect("the daemon answers or closes")
            .ok()?;
        (read > 0).then(|| serde_json::from_str(&line).expect("the daemon sends JSON"))
    }

    async fn handshaken(home: &TestHome) -> RawPeer {
        let mut peer = RawPeer::connect(home).await;
        peer.send_line(br#"{"protocol_versions":[1]}"#).await;
        let welcome = peer.read_frame().await.unwrap();
        assert_eq!(welcome["outcome"], "accepted", "{welcome}");
        peer
    }
}

#[tokio::test]
async fn a_client_sharing_no_version_is_refused_with_the_daemons_versions() {
    let home = TestHome::new();
    let _daemon = home.start();
    let mut peer = RawPeer::connect(&home).await;

    peer.send_line(br#"{"protocol_versions":[99]}"#).await;

    let refusal = peer.read_frame().await.unwrap();
    assert_eq!(
        (
            &refusal["outcome"],
            &refusal["reason"],
            &refusal["supported_versions"]
        ),
        (&json!("refused"), &json!("version-mismatch"), &json!([1]))
    );
    assert_eq!(peer.read_frame().await, None, "the connection closes");
}

#[tokio::test]
async fn a_first_frame_that_is_not_a_hello_is_refused() {
    let home = TestHome::new();
    let _daemon = home.start();
    let mut peer = RawPeer::connect(&home).await;

    peer.send_line(b"good morning").await;

    let refusal = peer.read_frame().await.unwrap();
    assert_eq!(refusal["reason"], "malformed-hello", "{refusal}");
    assert_eq!(peer.read_frame().await, None);
}

#[tokio::test]
async fn subscribe_on_a_connection_without_the_change_feed_is_not_negotiated() {
    let home = TestHome::new();
    let _daemon = home.start();
    let connection = home.connect().await;

    let result = connection
        .request(
            Command::Subscribe {
                from: FeedPosition::Snapshot,
            },
            GENEROUS,
        )
        .await;

    assert!(
        matches!(
            &result,
            Err(ClientError::Daemon(ProtocolError {
                code: ErrorCode::NotNegotiated,
                ..
            }))
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn an_unknown_command_is_answered_and_the_connection_stays_usable() {
    let home = TestHome::new();
    let _daemon = home.start();
    let mut peer = RawPeer::handshaken(&home).await;

    peer.send_line(br#"{"id":7,"command":{"kind":"launch-rocket"}}"#)
        .await;
    let refused = peer.read_frame().await.unwrap();
    peer.send_line(br#"{"id":8,"command":{"kind":"ping"}}"#)
        .await;
    let answered = peer.read_frame().await.unwrap();

    assert_eq!(
        (&refused["id"], &refused["outcome"]["error"]["code"]),
        (&json!(7), &json!("unknown-command"))
    );
    assert_eq!(
        serde_json::from_value::<Reply>(answered["outcome"]["ok"].clone()).unwrap(),
        Reply::Pong
    );
}

#[tokio::test]
async fn a_frame_without_a_readable_id_gets_a_malformed_frame_answer() {
    let home = TestHome::new();
    let _daemon = home.start();
    let mut peer = RawPeer::handshaken(&home).await;

    peer.send_line(b"[not, json").await;

    let answer = peer.read_frame().await.unwrap();
    assert_eq!(answer["type"], "malformed-frame", "{answer}");
}

#[tokio::test]
async fn an_oversized_frame_is_answered_and_closes_the_connection() {
    let home = TestHome::new();
    let _daemon = home.start();
    let mut peer = RawPeer::handshaken(&home).await;

    // The daemon stops reading at the limit, so the rest of the line may meet a closed socket.
    let _ = peer
        .writer
        .write_all(&vec![b' '; MAX_FRAME_BYTES + 1])
        .await;

    let answer = peer.read_frame().await.unwrap();
    assert_eq!(answer["type"], "malformed-frame", "{answer}");
    assert_eq!(peer.read_frame().await, None);
}

#[tokio::test]
async fn an_oversized_hello_is_refused_as_malformed() {
    let home = TestHome::new();
    let _daemon = home.start();
    let mut peer = RawPeer::connect(&home).await;

    let _ = peer
        .writer
        .write_all(&vec![b' '; MAX_FRAME_BYTES + 1])
        .await;

    let refusal = peer.read_frame().await.unwrap();
    assert_eq!(refusal["reason"], "malformed-hello", "{refusal}");
    assert_eq!(peer.read_frame().await, None);
}

#[tokio::test]
async fn a_hello_that_is_not_utf8_is_refused_instead_of_repaired() {
    let home = TestHome::new();
    let _daemon = home.start();
    let mut peer = RawPeer::connect(&home).await;

    peer.send_line(b"{\"protocol_versions\":[1],\"note\":\"\xff\"}")
        .await;

    let refusal = peer.read_frame().await.unwrap();
    assert_eq!(
        (&refusal["outcome"], &refusal["reason"]),
        (&json!("refused"), &json!("malformed-hello"))
    );
}
