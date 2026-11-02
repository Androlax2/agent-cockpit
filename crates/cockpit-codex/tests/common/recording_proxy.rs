//! Test-only transparent WebSocket proxy. It never creates an RPC or approval response.

use std::io::{Read, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Direction {
    Client,
    Server,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub connection: usize,
    pub direction: Direction,
    pub body: Value,
    pub disconnected: bool,
}

pub struct RecordingProxy {
    path: PathBuf,
    entries: Arc<Mutex<Vec<Entry>>>,
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<Vec<UnixStream>>>,
    worker: Option<JoinHandle<()>>,
}

impl RecordingProxy {
    pub fn start(path: &Path, upstream: &Path) -> Self {
        let listener = UnixListener::bind(path).unwrap();
        listener.set_nonblocking(true).unwrap();
        let upstream = upstream.to_owned();
        let entries = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let worker = {
            let entries = Arc::clone(&entries);
            let stop = Arc::clone(&stop);
            let sockets = Arc::clone(&sockets);
            std::thread::spawn(move || {
                let mut workers = Vec::new();
                let mut connection = 0;
                while !stop.load(Ordering::SeqCst) {
                    let client = match listener.accept() {
                        Ok((stream, _)) => stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        Err(error) => panic!("proxy accept: {error}"),
                    };
                    let server = UnixStream::connect(&upstream).unwrap();
                    {
                        let mut sockets = sockets.lock().unwrap();
                        sockets.extend([client.try_clone().unwrap(), server.try_clone().unwrap()]);
                    }
                    for (source, destination, direction) in [
                        (
                            client.try_clone().unwrap(),
                            server.try_clone().unwrap(),
                            Direction::Client,
                        ),
                        (server, client, Direction::Server),
                    ] {
                        let entries = Arc::clone(&entries);
                        workers.push(std::thread::spawn(move || {
                            forward(source, destination, connection, direction, &entries);
                        }));
                    }
                    connection += 1;
                }
                for socket in sockets.lock().unwrap().iter() {
                    let _ = socket.shutdown(Shutdown::Both);
                }
                for worker in workers {
                    worker.join().unwrap();
                }
            })
        };
        Self {
            path: path.to_owned(),
            entries,
            stop,
            sockets,
            worker: Some(worker),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> Vec<Entry> {
        self.entries.lock().unwrap().clone()
    }

    pub fn wait_for(&self, found: impl Fn(&[Entry]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !found(&self.entries()) {
            assert!(Instant::now() < deadline, "proxy observation timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for RecordingProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        for socket in self.sockets.lock().unwrap().iter() {
            let _ = socket.shutdown(Shutdown::Both);
        }
        let result = self.worker.take().unwrap().join();
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}

fn forward(
    mut source: UnixStream,
    mut destination: UnixStream,
    connection: usize,
    direction: Direction,
    entries: &Mutex<Vec<Entry>>,
) {
    source
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    destination
        .set_write_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut decoder = Decoder::default();
    let mut buffer = [0; 8192];
    while let Ok(count) = source.read(&mut buffer) {
        if count == 0 {
            break;
        }
        for body in decoder.feed(&buffer[..count]) {
            entries.lock().unwrap().push(Entry {
                connection,
                direction,
                body,
                disconnected: false,
            });
        }
        if destination.write_all(&buffer[..count]).is_err() {
            break;
        }
    }
    let _ = source.shutdown(Shutdown::Both);
    let _ = destination.shutdown(Shutdown::Both);
    entries.lock().unwrap().push(Entry {
        connection,
        direction,
        body: Value::Null,
        disconnected: true,
    });
}

#[derive(Default)]
struct Decoder {
    handshake: bool,
    unread: Vec<u8>,
    text: Vec<u8>,
}

impl Decoder {
    fn feed(&mut self, bytes: &[u8]) -> Vec<Value> {
        self.unread.extend_from_slice(bytes);
        if !self.handshake {
            let Some(end) = self.unread.windows(4).position(|part| part == b"\r\n\r\n") else {
                assert!(self.unread.len() < 64 * 1024, "oversized handshake");
                return Vec::new();
            };
            self.unread.drain(..end + 4);
            self.handshake = true;
        }
        let mut messages = Vec::new();
        while self.unread.len() >= 2 {
            let head = &self.unread;
            assert_eq!(
                head[0] & 0x70,
                0,
                "compressed frames are not supported by this recorder"
            );
            let masked = head[1] & 0x80 != 0;
            let (length, offset) = match head[1] & 0x7f {
                126 if head.len() < 4 => break,
                126 => (
                    u64::from(u16::from_be_bytes(head[2..4].try_into().unwrap())),
                    4,
                ),
                127 if head.len() < 10 => break,
                127 => (u64::from_be_bytes(head[2..10].try_into().unwrap()), 10),
                short => (u64::from(short), 2),
            };
            assert!(length <= 16 * 1024 * 1024, "oversized frame");
            let payload_at = offset + if masked { 4 } else { 0 };
            let end = payload_at + length as usize;
            if head.len() < end {
                break;
            }
            let mut payload = head[payload_at..end].to_vec();
            if masked {
                for (index, byte) in payload.iter_mut().enumerate() {
                    *byte ^= head[offset + index % 4];
                }
            }
            if matches!(head[0] & 0x0f, 0 | 1) {
                self.text.extend(payload);
                assert!(self.text.len() <= 16 * 1024 * 1024, "oversized message");
                if head[0] & 0x80 != 0 {
                    messages.push(serde_json::from_slice(&self.text).expect("JSON text frame"));
                    self.text.clear();
                }
            }
            self.unread.drain(..end);
        }
        messages
    }
}

#[test]
fn fragmented_requests_and_masked_responses_keep_their_ids() {
    let mut decoder = Decoder::default();
    assert!(decoder.feed(b"HTTP/1.1 101\r\n").is_empty());
    let request = br#"{"id":"approval-1","method":"item/commandExecution/requestApproval"}"#;
    let mut frames = b"\r\n".to_vec();
    frames.extend([0x01, 9]);
    frames.extend(&request[..9]);
    frames.extend([0x89, 1, b'p']);
    frames.extend([0x80, (request.len() - 9) as u8]);
    frames.extend(&request[9..]);
    let mut messages = Vec::new();
    for byte in frames {
        messages.extend(decoder.feed(&[byte]));
    }
    assert_eq!(
        messages,
        [serde_json::from_slice::<Value>(request).unwrap()]
    );
    let reply = br#"{"id":"approval-1","result":{"decision":"decline"}}"#;
    let mut frame = vec![0x81, 0x80 | reply.len() as u8, 1, 2, 3, 4];
    frame.extend(
        reply
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ [1, 2, 3, 4][index % 4]),
    );
    assert_eq!(
        decoder.feed(&frame),
        [serde_json::from_slice::<Value>(reply).unwrap()]
    );
}
