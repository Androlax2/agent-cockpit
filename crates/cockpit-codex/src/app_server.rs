//! A client of Codex's app-server on its unix socket: JSON-RPC in WebSocket text frames, as
//! `codex app-server --listen unix://PATH` speaks it (measured on 0.161.0). Only what
//! delivery and the session launcher use: one request at a time, and the notifications that
//! arrive meanwhile. Written on the standard library: adapters depend on
//! `cockpit-provider-api` only, and a WebSocket crate would be a new dependency for a few
//! frames.

use std::collections::VecDeque;
use std::fmt;
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};

/// Any key does: the server only echoes its digest, and the socket is local.
const HANDSHAKE_KEY: &str = "Y29ja3BpdC1jb2RleC1rZXk=";
/// The longest frame this client accepts; Codex's answers to its calls are a few kilobytes.
const MAX_FRAME_BYTES: u64 = 16 * 1024 * 1024;

const TEXT: u8 = 0x1;
const CONTINUATION: u8 = 0x0;
const CLOSE: u8 = 0x8;
const PING: u8 = 0x9;
const PONG: u8 = 0xA;

/// One connection to the app-server, initialized.
pub struct AppServer {
    stream: UnixStream,
    unread: Vec<u8>,
    next_id: u64,
    notifications: VecDeque<Notification>,
}

/// A message the server sent without being asked.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub method: String,
    pub params: Value,
}

impl AppServer {
    /// Connects to the server listening on `socket` and initializes the connection as
    /// `client`. Every read and write gives up after `timeout`.
    /// Inside the sandbox only: the daemon opens the socket through its checked
    /// [`cockpit_provider_api::SessionSockets`] and calls [`AppServer::over`].
    pub fn connect(socket: &Path, client: &str, timeout: Duration) -> Result<AppServer, RpcError> {
        let stream = UnixStream::connect(socket).map_err(|error| {
            RpcError::new(format!(
                "cannot reach Codex's app-server at {}: {error}",
                socket.display()
            ))
        })?;
        AppServer::over(stream, client, timeout)
    }

    /// Initializes the connection `stream` as `client`. Every read and write gives up after
    /// `timeout`.
    pub fn over(
        stream: UnixStream,
        client: &str,
        timeout: Duration,
    ) -> Result<AppServer, RpcError> {
        let mut server = AppServer {
            stream,
            unread: Vec::new(),
            next_id: 1,
            notifications: VecDeque::new(),
        };
        server.set_timeout(Some(timeout))?;
        server.handshake()?;
        server.call(
            "initialize",
            json!({"clientInfo": {"name": client, "version": env!("CARGO_PKG_VERSION")}}),
        )?;
        server.send(&json!({"method": "initialized"}))?;
        Ok(server)
    }

    /// How long one read or write may take; `None` waits as long as the server stays.
    pub fn set_timeout(&mut self, timeout: Option<Duration>) -> Result<(), RpcError> {
        self.stream
            .set_read_timeout(timeout)
            .and_then(|()| self.stream.set_write_timeout(timeout))
            .map_err(|error| RpcError::new(format!("cannot set the socket's timeout: {error}")))
    }

    /// The result of `method` with `params`, or the server's error. Notifications that
    /// arrive first are kept for [`AppServer::next_notification`].
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"id": id, "method": method, "params": params}))?;
        loop {
            let message = self.receive()?;
            if message.get("method").is_some() {
                // A request from the server (an approval) is for the TUI, which answers it.
                if message.get("id").is_none() {
                    self.notifications.push_back(notification(message));
                }
                continue;
            }
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                let reason = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("no message");
                return Err(RpcError {
                    reason: format!("`{method}` failed: {reason}"),
                    is_refusal: true,
                });
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// The next notification, waiting for it.
    pub fn next_notification(&mut self) -> Result<Notification, RpcError> {
        loop {
            if let Some(notification) = self.notifications.pop_front() {
                return Ok(notification);
            }
            let message = self.receive()?;
            if message.get("method").is_some() && message.get("id").is_none() {
                return Ok(notification(message));
            }
        }
    }

    fn handshake(&mut self) -> Result<(), RpcError> {
        let request = format!(
            "GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {HANDSHAKE_KEY}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        self.write_all(request.as_bytes())?;
        let end = loop {
            if let Some(end) = find(&self.unread, b"\r\n\r\n") {
                break end;
            }
            self.read_more()?;
        };
        let head = String::from_utf8_lossy(&self.unread[..end]).into_owned();
        self.unread.drain(..end + 4);
        let status = head.lines().next().unwrap_or_default();
        if !status.starts_with("HTTP/1.1 101") {
            return Err(RpcError::new(format!(
                "the app-server refused the connection: {status:?}"
            )));
        }
        Ok(())
    }

    fn send(&mut self, message: &Value) -> Result<(), RpcError> {
        let frame = masked_frame(TEXT, message.to_string().as_bytes());
        self.write_all(&frame)
    }

    /// The next JSON message, answering pings on the way.
    fn receive(&mut self) -> Result<Value, RpcError> {
        let mut text = Vec::new();
        loop {
            let (is_final, opcode, payload) = self.read_frame()?;
            match opcode {
                TEXT | CONTINUATION => text.extend_from_slice(&payload),
                PING => {
                    let pong = masked_frame(PONG, &payload);
                    self.write_all(&pong)?;
                    continue;
                }
                PONG => continue,
                CLOSE => return Err(RpcError::new("the app-server closed the connection")),
                other => {
                    return Err(RpcError::new(format!(
                        "the app-server sent an unknown frame type {other:#x}"
                    )));
                }
            }
            if is_final {
                return serde_json::from_slice(&text).map_err(|error| {
                    RpcError::new(format!(
                        "the app-server sent a message that is not JSON: {error}"
                    ))
                });
            }
        }
    }

    fn read_frame(&mut self) -> Result<(bool, u8, Vec<u8>), RpcError> {
        let header = self.take(2)?;
        let is_final = header[0] & 0x80 != 0;
        let opcode = header[0] & 0x0F;
        let is_masked = header[1] & 0x80 != 0;
        let length = match header[1] & 0x7F {
            126 => u64::from(u16::from_be_bytes(
                self.take(2)?.try_into().expect("two bytes"),
            )),
            127 => u64::from_be_bytes(self.take(8)?.try_into().expect("eight bytes")),
            short => u64::from(short),
        };
        if length > MAX_FRAME_BYTES {
            return Err(RpcError::new(format!(
                "the app-server sent a frame of {length} bytes"
            )));
        }
        let mask = if is_masked { Some(self.take(4)?) } else { None };
        let mut payload = self.take(length as usize)?;
        if let Some(mask) = mask {
            for (index, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[index % 4];
            }
        }
        Ok((is_final, opcode, payload))
    }

    fn take(&mut self, count: usize) -> Result<Vec<u8>, RpcError> {
        while self.unread.len() < count {
            self.read_more()?;
        }
        Ok(self.unread.drain(..count).collect())
    }

    fn read_more(&mut self) -> Result<(), RpcError> {
        let mut chunk = [0u8; 8192];
        let read = self
            .stream
            .read(&mut chunk)
            .map_err(|error| RpcError::new(format!("cannot read from the app-server: {error}")))?;
        if read == 0 {
            return Err(RpcError::new("the app-server closed the connection"));
        }
        self.unread.extend_from_slice(&chunk[..read]);
        Ok(())
    }

    fn write_all(&mut self, bytes: &[u8]) -> Result<(), RpcError> {
        self.stream
            .write_all(bytes)
            .map_err(|error| RpcError::new(format!("cannot write to the app-server: {error}")))
    }
}

fn notification(mut message: Value) -> Notification {
    Notification {
        method: message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        params: message
            .get_mut("params")
            .map(Value::take)
            .unwrap_or(Value::Null),
    }
}

/// A client frame: final, masked as the protocol requires of clients.
fn masked_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut frame = vec![0x80 | opcode];
    match payload.len() {
        length if length < 126 => frame.push(0x80 | length as u8),
        length if length <= usize::from(u16::MAX) => {
            frame.push(0x80 | 126);
            frame.extend_from_slice(&(length as u16).to_be_bytes());
        }
        length => {
            frame.push(0x80 | 127);
            frame.extend_from_slice(&(length as u64).to_be_bytes());
        }
    }
    let mask = mask_key();
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4]),
    );
    frame
}

/// A fresh mask per frame; it guards proxies, not secrets, so the standard library's
/// per-process random hasher seed is enough.
fn mask_key() -> [u8; 4] {
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let bytes = random.to_le_bytes();
    [bytes[0], bytes[1], bytes[2], bytes[3]]
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The app-server could not be reached or refused a call; `reason` says which, for the owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub reason: String,
    /// The server answered the call with an error, so the connection still works.
    pub is_refusal: bool,
}

impl RpcError {
    fn new(reason: impl Into<String>) -> RpcError {
        RpcError {
            reason: reason.into(),
            is_refusal: false,
        }
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for RpcError {}
