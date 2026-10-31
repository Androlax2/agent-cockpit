//! A scripted daemon socket: it answers the handshake and scripted requests, for tests that
//! drive `cockpit` without the daemon (the crate graph keeps frontends from building it).
#![allow(
    dead_code,
    reason = "each test file uses a different part of the fixture"
)]

use std::path::Path;

use cockpit_protocol::{
    Command, HandshakeReply, Outcome, ProtocolVersion, Request, Response, ServerFrame, Welcome,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// Accepts one connection on `socket`, answers the handshake, then answers its one request
/// with `outcome`, and returns the command it received.
pub fn scripted_daemon(socket: &Path, outcome: Outcome) -> tokio::task::JoinHandle<Command> {
    let conversation = scripted_conversation(socket, vec![outcome]);
    tokio::spawn(async move { conversation.await.unwrap().pop().expect("a request") })
}

/// Accepts one connection on `socket`, answers the handshake, then each request with the
/// next of `outcomes`, and returns the commands it received.
pub fn scripted_conversation(
    socket: &Path,
    outcomes: Vec<Outcome>,
) -> tokio::task::JoinHandle<Vec<Command>> {
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read_half, mut write_half) = stream.into_split();
        let mut lines = BufReader::new(read_half).lines();
        lines.next_line().await.unwrap().expect("a hello");
        let welcome = HandshakeReply::Accepted(Welcome {
            protocol_version: ProtocolVersion(1),
            capabilities: Vec::new(),
        });
        send(&mut write_half, serde_json::to_value(welcome).unwrap()).await;
        let mut received = Vec::new();
        for outcome in outcomes {
            let Some(line) = lines.next_line().await.unwrap() else {
                break;
            };
            let request: Request = serde_json::from_str(&line).unwrap();
            received.push(request.command);
            let response = ServerFrame::Response(Response {
                id: request.id,
                outcome,
            });
            send(&mut write_half, serde_json::to_value(response).unwrap()).await;
        }
        received
    })
}

pub async fn send(writer: &mut tokio::net::unix::OwnedWriteHalf, frame: serde_json::Value) {
    let mut line = serde_json::to_vec(&frame).unwrap();
    line.push(b'\n');
    writer.write_all(&line).await.unwrap();
}

pub fn text(bytes: &[u8]) -> String {
    String::from_utf8(bytes.to_vec()).unwrap()
}
