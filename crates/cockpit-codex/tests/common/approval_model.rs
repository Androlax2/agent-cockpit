//! A loopback Responses fixture: ready, then a gated approval request. No credentials.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub struct Model {
    pub url: String,
    calls: Arc<AtomicUsize>,
    released: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Model {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let released = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let calls = Arc::clone(&calls);
            let released = Arc::clone(&released);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream
                                .set_read_timeout(Some(Duration::from_secs(5)))
                                .unwrap();
                            stream
                                .set_write_timeout(Some(Duration::from_secs(5)))
                                .unwrap();
                            let (header, request) = read_request(&mut stream);
                            assert!(
                                header.starts_with("POST /v1/responses "),
                                "unexpected model endpoint"
                            );
                            // The TUI creates a tool-less helper thread to generate a title.
                            // It is not the conversation whose approvals this probe measures.
                            let helper = request["tools"].as_array().is_none_or(Vec::is_empty);
                            let call = if helper {
                                0
                            } else {
                                calls.fetch_add(1, Ordering::SeqCst) + 1
                            };
                            assert!(call <= 4, "unexpected repeated model requests");
                            if call == 2 {
                                let deadline = Instant::now() + Duration::from_secs(45);
                                while !released.load(Ordering::SeqCst)
                                    && !stop.load(Ordering::SeqCst)
                                {
                                    assert!(
                                        Instant::now() < deadline,
                                        "approval fixture was never released"
                                    );
                                    std::thread::sleep(Duration::from_millis(20));
                                }
                            }
                            let body = response(call);
                            let reply = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = stream.write_all(reply.as_bytes());
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10))
                        }
                        Err(error) => panic!("model accept: {error}"),
                    }
                }
            })
        };
        Self {
            url,
            calls,
            released,
            stop,
            worker: Some(worker),
        }
    }

    pub fn wait_for_pending_tool(&self) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.calls.load(Ordering::SeqCst) < 2 {
            assert!(Instant::now() < deadline, "model received no second turn");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn release_tool(&self) {
        self.released.store(true, Ordering::SeqCst);
    }
}

impl Drop for Model {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let result = self.worker.take().unwrap().join();
        if !std::thread::panicking() {
            result.unwrap();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> (String, Value) {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = stream.read(&mut buffer).unwrap();
        assert!(count > 0, "incomplete model request");
        bytes.extend_from_slice(&buffer[..count]);
        assert!(bytes.len() <= 8 * 1024 * 1024, "oversized model request");
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let header = String::from_utf8(bytes[..end].to_vec()).unwrap();
            let length: usize = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .expect("content-length");
            if bytes.len() >= end + 4 + length {
                return (
                    header,
                    serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap(),
                );
            }
        }
    }
}

fn response(call: usize) -> String {
    let id = format!("fixture-response-{call}");
    let item = if call >= 2 {
        // Steering may discard an in-flight response and ask the model again.
        json!({"type":"function_call", "call_id":format!("fixture-command-{call}"), "name":"exec_command", "arguments":json!({
            "cmd":"/usr/bin/printf cockpit-approval-fixture", "sandbox_permissions":"require_escalated",
            "justification":"Conformance fixture: refuse this harmless command.", "max_output_tokens":100
        }).to_string()})
    } else {
        json!({"type":"message", "role":"assistant", "id":format!("fixture-message-{call}"),
            "content":[{"type":"output_text","text": if call == 0 { r#"{"title":"Approval fixture"}"# } else { "Fixture ready." }}]})
    };
    sse(&[
        json!({"type":"response.created", "response":{"id":id}}),
        json!({"type":"response.output_item.done", "item":item}),
        json!({"type":"response.completed", "response":{"id":id,
            "usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0}}}),
    ])
}

fn sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap()
            )
        })
        .collect()
}
