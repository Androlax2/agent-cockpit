//! A fake Codex app-server on a unix socket: the WebSocket handshake, JSON-RPC in text
//! frames as 0.161.0 speaks it, scripted answers, every request recorded, and notifications
//! pushed to every connection.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The answer to one request: its result, or an error message.
pub type Answer = Result<Value, String>;
type Script = dyn Fn(&str, &Value) -> Answer + Send + Sync;

pub struct FakeAppServer {
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    connections: Arc<Mutex<Vec<UnixStream>>>,
}

impl FakeAppServer {
    /// Listens on `socket`; `script` answers each request but `initialize`.
    pub fn start(
        socket: &Path,
        script: impl Fn(&str, &Value) -> Answer + Send + Sync + 'static,
    ) -> FakeAppServer {
        let listener = UnixListener::bind(socket).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(Mutex::new(Vec::new()));
        let script: Arc<Script> = Arc::new(script);
        {
            let requests = Arc::clone(&requests);
            let connections = Arc::clone(&connections);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { return };
                    let requests = Arc::clone(&requests);
                    let connections = Arc::clone(&connections);
                    let script = Arc::clone(&script);
                    std::thread::spawn(move || serve(stream, &requests, &connections, &*script));
                }
            });
        }
        FakeAppServer {
            requests,
            connections,
        }
    }

    /// Every request received but `initialize`, oldest first: method and params.
    pub fn requests(&self) -> Vec<(String, Value)> {
        self.requests.lock().unwrap().clone()
    }

    /// The methods of [`FakeAppServer::requests`].
    pub fn methods(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|(method, _)| method)
            .collect()
    }

    /// Waits until `count` connections have initialized.
    pub fn wait_for_connections(&self, count: usize) {
        let ends_at = Instant::now() + Duration::from_secs(10);
        while self.connections.lock().unwrap().len() < count {
            assert!(Instant::now() < ends_at, "no {count} connections");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Sends a notification to every initialized connection.
    pub fn notify(&self, method: &str, params: Value) {
        let frame = server_frame(&json!({"method": method, "params": params}).to_string());
        for connection in self.connections.lock().unwrap().iter_mut() {
            let _ = connection.write_all(&frame);
        }
    }
}

fn serve(
    mut stream: UnixStream,
    requests: &Mutex<Vec<(String, Value)>>,
    connections: &Mutex<Vec<UnixStream>>,
    script: &Script,
) {
    let mut unread = Vec::new();
    while !unread.windows(4).any(|window| window == b"\r\n\r\n") {
        if !read_more(&mut stream, &mut unread) {
            return;
        }
    }
    let end = unread
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap();
    unread.drain(..end + 4);
    stream
        .write_all(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
              Sec-WebSocket-Accept: fake\r\n\r\n",
        )
        .unwrap();
    while let Some(text) = read_client_frame(&mut stream, &mut unread) {
        let message: Value = serde_json::from_str(&text).unwrap();
        let method = message["method"].as_str().unwrap_or_default().to_owned();
        let Some(id) = message.get("id").cloned() else {
            if method == "initialized" {
                connections
                    .lock()
                    .unwrap()
                    .push(stream.try_clone().unwrap());
            }
            continue;
        };
        let answer = if method == "initialize" {
            Ok(json!({"userAgent": "fake/0.161.0"}))
        } else {
            requests
                .lock()
                .unwrap()
                .push((method.clone(), message["params"].clone()));
            script(&method, &message["params"])
        };
        let reply = match answer {
            Ok(result) => json!({"id": id, "result": result}),
            Err(reason) => json!({"id": id, "error": {"code": -32600, "message": reason}}),
        };
        if stream.write_all(&server_frame(&reply.to_string())).is_err() {
            return;
        }
    }
}

fn read_more(stream: &mut UnixStream, unread: &mut Vec<u8>) -> bool {
    let mut chunk = [0u8; 8192];
    match stream.read(&mut chunk) {
        Ok(0) | Err(_) => false,
        Ok(read) => {
            unread.extend_from_slice(&chunk[..read]);
            true
        }
    }
}

fn take(stream: &mut UnixStream, unread: &mut Vec<u8>, count: usize) -> Option<Vec<u8>> {
    while unread.len() < count {
        if !read_more(stream, unread) {
            return None;
        }
    }
    Some(unread.drain(..count).collect())
}

/// One masked text frame from the client; panics on an unmasked one, which the protocol
/// forbids clients.
fn read_client_frame(stream: &mut UnixStream, unread: &mut Vec<u8>) -> Option<String> {
    let header = take(stream, unread, 2)?;
    assert_eq!(header[0], 0x81, "a final text frame");
    assert!(header[1] & 0x80 != 0, "client frames are masked");
    let length = match header[1] & 0x7F {
        126 => usize::from(u16::from_be_bytes(
            take(stream, unread, 2)?.try_into().unwrap(),
        )),
        127 => u64::from_be_bytes(take(stream, unread, 8)?.try_into().unwrap()) as usize,
        short => usize::from(short),
    };
    let mask = take(stream, unread, 4)?;
    let payload: Vec<u8> = take(stream, unread, length)?
        .iter()
        .enumerate()
        .map(|(index, byte)| byte ^ mask[index % 4])
        .collect();
    Some(String::from_utf8(payload).unwrap())
}

/// An unmasked final text frame, as a server sends.
fn server_frame(text: &str) -> Vec<u8> {
    let mut frame = vec![0x81];
    match text.len() {
        length if length < 126 => frame.push(length as u8),
        length if length <= usize::from(u16::MAX) => {
            frame.push(126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            frame.push(127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    frame.extend_from_slice(text.as_bytes());
    frame
}

/// The answer to `thread/turns/list` whose last turn is `id` with `status`.
pub fn last_turn(id: &str, status: &str) -> Answer {
    Ok(json!({"data": [{"id": id, "items": [], "status": status, "error": null}]}))
}
