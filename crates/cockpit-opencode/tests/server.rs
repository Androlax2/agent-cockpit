//! A session's server API against a fake OpenCode server on the loopback interface: it
//! answers each request with what OpenCode 1.19.5 answers, and records what it was sent.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cockpit_opencode::{Invocation, NativeCommand, Server};
use cockpit_provider_api::ConversationId;

const PASSWORD: &str = "5f0c2d1e9a8b7c6d5e4f3a2b1c0d9e8f";
const SESSION: &str = "ses_1c4f2e8a3ffeJ7qkR2mWbXzD0v";

#[derive(Debug, Clone)]
struct Received {
    method: String,
    path: String,
    authorization: Option<String>,
    body: String,
}

/// One canned answer per request, in order: status, extra headers, body.
struct FakeServer {
    port: u16,
    received: Arc<Mutex<Vec<Received>>>,
}

impl FakeServer {
    fn answering(answers: Vec<(u16, &'static str, String)>) -> FakeServer {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for (status, headers, body) in answers {
                let (mut stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut words = request_line.split_whitespace();
                let method = words.next().unwrap().to_owned();
                let path = words.next().unwrap().to_owned();
                let mut length = 0;
                let mut authorization = None;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    let line = line.trim_end();
                    if line.is_empty() {
                        break;
                    }
                    let (name, value) = line.split_once(": ").unwrap();
                    match name.to_ascii_lowercase().as_str() {
                        "content-length" => length = value.parse().unwrap(),
                        "authorization" => authorization = Some(value.to_owned()),
                        _ => {}
                    }
                }
                let mut request_body = vec![0; length];
                reader.read_exact(&mut request_body).unwrap();
                log.lock().unwrap().push(Received {
                    method,
                    path,
                    authorization,
                    body: String::from_utf8(request_body).unwrap(),
                });
                let framing = if headers.contains("chunked") {
                    String::new()
                } else {
                    format!("Content-Length: {}\r\n", body.len())
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\n{headers}{framing}Connection: close\r\n\r\n{body}"
                )
                .unwrap();
            }
        });
        FakeServer { port, received }
    }

    fn server(&self) -> Server {
        Server::new(self.port, PASSWORD.to_owned())
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

fn ok(body: &str) -> (u16, &'static str, String) {
    (200, "Content-Type: application/json\r\n", body.to_owned())
}

fn session() -> ConversationId {
    ConversationId::new(SESSION).unwrap()
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn every_request_carries_the_sessions_password() {
    let fake = FakeServer::answering(vec![ok(r#"{"healthy":true,"version":"1.19.5"}"#)]);

    assert_eq!(fake.server().health().unwrap(), "1.19.5");

    let received = &fake.received()[0];
    assert_eq!(
        (received.method.as_str(), received.path.as_str()),
        ("GET", "/global/health")
    );
    // base64("opencode:5f0c2d1e9a8b7c6d5e4f3a2b1c0d9e8f")
    assert_eq!(
        received.authorization.as_deref(),
        Some("Basic b3BlbmNvZGU6NWYwYzJkMWU5YThiN2M2ZDVlNGYzYTJiMWMwZDllOGY=")
    );
}

#[test]
fn a_refused_password_says_so() {
    let fake = FakeServer::answering(vec![(401, "", "Unauthorized".to_owned())]);

    let error = fake.server().health().unwrap_err();

    assert!(
        error.reason.contains("refused the session's password"),
        "{error}"
    );
}

#[test]
fn native_commands_are_listed_with_their_source() {
    let fake = FakeServer::answering(vec![ok(r#"[
        {"name":"init","description":"create/update AGENTS.md","source":"command","template":"...","hints":["$ARGUMENTS"]},
        {"name":"review","template":"...","hints":[]},
        {"name":"github:triage","description":"Triage an issue","source":"mcp","template":{},"hints":[]}
    ]"#)]);

    let commands = fake.server().commands().unwrap();

    assert_eq!(
        commands,
        [
            NativeCommand {
                name: "init".to_owned(),
                description: Some("create/update AGENTS.md".to_owned()),
                source: Some("command".to_owned()),
            },
            NativeCommand {
                name: "review".to_owned(),
                description: None,
                source: None,
            },
            NativeCommand {
                name: "github:triage".to_owned(),
                description: Some("Triage an issue".to_owned()),
                source: Some("mcp".to_owned()),
            },
        ]
    );
    assert_eq!(fake.received()[0].path, "/command");
}

/// Bun answers a large JSON body in chunks.
#[test]
fn a_chunked_answer_is_read_whole() {
    let fake = FakeServer::answering(vec![(
        200,
        "Transfer-Encoding: chunked\r\n",
        "5\r\n[{\"na\r\n17;ext=1\r\nme\":\"init\",\"hints\":[]}]\r\n0\r\n\r\n".to_owned(),
    )]);

    let commands = fake.server().commands().unwrap();

    assert_eq!(commands[0].name, "init");
}

#[test]
fn a_command_is_invoked_in_its_session_at_its_variant() {
    let fake = FakeServer::answering(vec![ok(
        r#"{"info":{"id":"msg_e3b0c4421001Xh2pK8sLqW4nZr","role":"assistant"},"parts":[]}"#,
    )]);

    let invocation = fake
        .server()
        .invoke_command(
            &session(),
            "review",
            "src/parser.rs",
            Some("high"),
            Duration::from_secs(5),
        )
        .unwrap();

    assert_eq!(
        invocation,
        Invocation {
            message: "msg_e3b0c4421001Xh2pK8sLqW4nZr".to_owned()
        }
    );
    let received = &fake.received()[0];
    assert_eq!(received.method, "POST");
    assert_eq!(received.path, format!("/session/{SESSION}/command"));
    assert_eq!(
        json(&received.body),
        serde_json::json!({ "command": "review", "arguments": "src/parser.rs", "variant": "high" })
    );
}

#[test]
fn an_unknown_command_reports_opencodes_answer() {
    let fake = FakeServer::answering(vec![(
        400,
        "",
        r#"{"_tag":"CommandNotFoundError","command":"nope","message":"Command not found: nope"}"#
            .to_owned(),
    )]);

    let error = fake
        .server()
        .invoke_command(&session(), "nope", "", None, Duration::from_secs(5))
        .unwrap_err();

    assert!(
        error.reason.contains("400") && error.reason.contains("Command not found: nope"),
        "{error}"
    );
}

#[test]
fn an_inbox_message_is_delivered_as_written() {
    let fake = FakeServer::answering(vec![(204, "", String::new())]);

    fake.server()
        .deliver(&session(), "/compact is a word here, as is @you", None)
        .unwrap();

    let received = &fake.received()[0];
    assert_eq!(received.path, format!("/session/{SESSION}/prompt_async"));
    assert_eq!(
        json(&received.body),
        serde_json::json!({ "parts": [{ "type": "text", "text": "/compact is a word here, as is @you" }] })
    );
}

#[test]
fn an_inbox_message_goes_at_its_variant() {
    let fake = FakeServer::answering(vec![(204, "", String::new())]);

    fake.server()
        .deliver(&session(), "go on", Some("max"))
        .unwrap();

    assert_eq!(json(&fake.received()[0].body)["variant"], "max");
}

#[test]
fn a_turn_is_aborted_in_its_session() {
    let fake = FakeServer::answering(vec![ok("true")]);

    fake.server().abort(&session()).unwrap();

    let received = &fake.received()[0];
    assert_eq!(
        (received.method.as_str(), received.path.as_str()),
        ("POST", format!("/session/{SESSION}/abort").as_str())
    );
}

#[test]
fn a_models_variants_are_listed_sorted() {
    let fake = FakeServer::answering(vec![ok(r#"{
        "providers": [
            {"id":"openai","models":{"gpt-6":{"variants":{}}}},
            {"id":"anthropic","models":{"claude-sonnet-5-5":{"id":"claude-sonnet-5-5","variants":{"max":{},"high":{"thinking":{}}}}}}
        ],
        "default": {"anthropic":"claude-sonnet-5-5"}
    }"#)]);

    let variants = fake
        .server()
        .variants("anthropic", "claude-sonnet-5-5")
        .unwrap();

    assert_eq!(variants, ["high", "max"]);
    assert_eq!(fake.received()[0].path, "/config/providers");
}

#[test]
fn an_unknown_model_has_no_variants_to_list() {
    let fake = FakeServer::answering(vec![ok(r#"{"providers":[],"default":{}}"#)]);

    let error = fake.server().variants("anthropic", "nope").unwrap_err();

    assert_eq!(error.reason, "OpenCode has no model anthropic/nope");
}

/// Nothing reaches the server: the refusal comes before any connection.
#[test]
fn a_session_id_or_variant_that_could_leave_its_place_is_refused() {
    let server = Server::new(1, PASSWORD.to_owned());
    let escaping = ConversationId::new("ses_x/../../config").unwrap();

    assert!(server.deliver(&escaping, "hi", None).is_err());
    assert!(
        server
            .abort(&ConversationId::new("other-1").unwrap())
            .is_err()
    );
    assert!(server.deliver(&session(), "hi", Some("high#x")).is_err());
    assert!(
        server
            .invoke_command(&session(), "two words", "", None, Duration::from_secs(1))
            .is_err()
    );
}

#[test]
fn a_server_that_never_answers_is_given_up_on_at_the_deadline() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let started = std::time::Instant::now();

    let error = Server::new(port, PASSWORD.to_owned())
        .invoke_command(&session(), "review", "", None, Duration::from_millis(300))
        .unwrap_err();

    assert!(error.reason.contains("no answer within 300 ms"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(5));
    drop(listener);
}

#[test]
fn the_password_never_shows_in_debug_output() {
    let shown = format!("{:?}", Server::new(4096, PASSWORD.to_owned()));

    assert!(!shown.contains(PASSWORD), "{shown}");
}
