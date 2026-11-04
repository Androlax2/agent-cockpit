//! Owner send/cancel at the real daemon socket, with a fake provider and isolated tmux.
#![cfg(target_os = "linux")]
mod candidate;
mod common;
mod messaging_fixture;
use cockpit_protocol::{Command, ErrorCode, Reply};
use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{
    Action, Activity, Capabilities, Capability, ConversationId, Delivered, DeliveryTarget, Key,
    LaunchCommand, LaunchRequest, Provider, ProviderError, ProviderName, TerminalInput,
};
use common::{GENEROUS, refused};
use messaging_fixture::Home;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
enum Transport {
    Terminal,
    Direct,
    Failure,
}
struct Agent {
    fake: FakeProvider,
    transport: Transport,
    calls: Mutex<Vec<(String, String)>>,
}
impl Agent {
    fn new(home: &Home, transport: Transport) -> Self {
        Self {
            fake: FakeProvider::new(
                ProviderName::new("fake").unwrap(),
                home.path().join("launches"),
            )
            .reading_messages(home.path().join("messages"))
            .reporting(Capability::Cancel),
            transport,
            calls: Mutex::new(Vec::new()),
        }
    }
}
impl Provider for Agent {
    fn name(&self) -> &ProviderName {
        self.fake.name()
    }
    fn capabilities(&self) -> Capabilities {
        self.fake.capabilities()
    }
    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        self.fake.launch(request)
    }
    fn deliver(
        &self,
        action: &Action<'_>,
        target: &DeliveryTarget<'_>,
        conversation: &ConversationId,
    ) -> Result<Delivered, ProviderError> {
        assert!(target.session_dir.join("agent-was-here").exists());
        match self.transport {
            Transport::Terminal => Ok(Delivered::Unsupported),
            Transport::Failure => Err(ProviderError {
                reason: "connection lost after writing".into(),
            }),
            Transport::Direct => {
                let (text, result) = match action {
                    Action::Submit(text) => (*text, Delivered::StartedTurn),
                    Action::Cancel => ("cancel", Delivered::Interrupted),
                    _ => unreachable!(),
                };
                self.calls
                    .lock()
                    .unwrap()
                    .push((conversation.to_string(), text.to_owned()));
                Ok(result)
            }
        }
    }
    fn input(
        &self,
        action: &Action<'_>,
        activity: Option<Activity>,
    ) -> Result<Vec<TerminalInput>, ProviderError> {
        assert!(
            matches!(self.transport, Transport::Terminal),
            "delivery failure or success must never type"
        );
        if matches!(action, Action::Cancel) {
            return Ok(vec![
                TerminalInput::Text("cancel".into()),
                TerminalInput::Key(Key::Enter),
            ]);
        }
        self.fake.input(action, activity)
    }
}
fn send(session: &str, message: &str) -> Command {
    Command::SessionSend {
        session: session.into(),
        message: message.into(),
    }
}
fn cancel(session: &str) -> Command {
    Command::SessionCancel {
        session: session.into(),
    }
}

#[tokio::test]
async fn owner_send_and_cancel_deliver_to_the_named_conversation() {
    let home = Home::new();
    let provider = Arc::new(Agent::new(&home, Transport::Direct));
    let _daemon = home.start(vec![provider.clone()]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;
    home.waiting(&session.id).await;
    for command in [send(&session.id, "hello\nworld"), cancel(&session.id)] {
        assert!(matches!(
            client.request(command, GENEROUS).await.unwrap(),
            Reply::SessionAction { .. }
        ));
    }
    assert_eq!(
        *provider.calls.lock().unwrap(),
        vec![
            ("conversation-1".into(), "hello\nworld".into()),
            ("conversation-1".into(), "cancel".into())
        ]
    );
    assert!(FakeProvider::messages(&home.path().join("messages")).is_empty());
}

#[tokio::test]
async fn the_exit_sweep_never_makes_a_live_session_refuse_a_send_as_busy() {
    let home = Home::new();
    let provider = Arc::new(Agent::new(&home, Transport::Direct));
    let _daemon = home.start(vec![provider]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;
    home.waiting(&session.id).await;

    // The daemon sweeps for exited agents every second (#632); sends span several sweeps.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        client
            .request(send(&session.id, "hello"), GENEROUS)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn unsupported_delivery_uses_the_providers_terminal_input_for_send_and_cancel() {
    let home = Home::new();
    let provider = Arc::new(Agent::new(&home, Transport::Terminal));
    let _daemon = home.start(vec![provider]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;
    home.waiting(&session.id).await;
    for command in [
        send(&session.id, "hello C-c ; $(false)"),
        cancel(&session.id),
    ] {
        client.request(command, GENEROUS).await.unwrap();
    }
    assert_eq!(
        home.messages(2).await,
        vec![
            (session.id.clone(), "hello C-c ; $(false)".into()),
            (session.id, "cancel".into())
        ]
    );
}

/// A prompt like Claude Code's (#730): a read of text is a paste and an Enter that comes in
/// the same read is dropped; only an Enter read on its own submits what the box holds, as
/// `<session> <line>` to the messages log. It touches `{ready}` once its terminal is raw.
const PASTE_DETECTING_PROMPT: &str = r#"#!/usr/bin/python3
import os, termios, tty
tty.setraw(0, termios.TCSANOW)
open('{ready}', 'w').close()
box = b''
while chunk := os.read(0, 65536):
    if chunk == b'\r':
        with open(os.environ['FAKE_AGENT_MESSAGES'], 'a') as log:
            log.write(os.environ['TORLEK_SESSION'] + ' ' + box.decode() + '\n')
        box = b''
    else:
        box += chunk.replace(b'\r', b'')
"#;

#[tokio::test]
async fn a_typed_message_is_submitted_by_an_agent_that_drops_an_enter_inside_a_paste() {
    let home = Home::new();
    let ready = home.path().join("prompt-ready");
    let program = home.path().join("paste-detecting-prompt");
    std::fs::write(
        &program,
        PASTE_DETECTING_PROMPT.replace("{ready}", ready.to_str().unwrap()),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut agent = Agent::new(&home, Transport::Terminal);
    agent.fake = agent.fake.running(program);
    let _daemon = home.start(vec![Arc::new(agent)]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;
    home.waiting(&session.id).await;
    common::once_logged(1, || ready.exists().then_some(()).into_iter().collect()).await;
    let answer = "Owner: A. Keep the old flag and rename it in the next release.";

    client
        .request(send(&session.id, answer), GENEROUS)
        .await
        .unwrap();

    assert_eq!(home.messages(1).await, vec![(session.id, answer.into())]);
}

#[tokio::test]
async fn a_delivery_error_never_falls_back_to_typing() {
    let home = Home::new();
    let provider = Arc::new(Agent::new(&home, Transport::Failure));
    let _daemon = home.start(vec![provider]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;
    home.waiting(&session.id).await;
    for command in [send(&session.id, "hello"), cancel(&session.id)] {
        let error = refused(client.request(command, GENEROUS).await);
        assert!(
            error.message.contains("connection lost after writing"),
            "{error:?}"
        );
    }
    assert!(FakeProvider::messages(&home.path().join("messages")).is_empty());
}

#[tokio::test]
async fn missing_capabilities_unknown_conversations_and_agent_requests_are_refused() {
    let home = Home::new();
    let plain = FakeProvider::new(
        ProviderName::new("plain").unwrap(),
        home.path().join("plain"),
    );
    let unnamed = FakeProvider::new(
        ProviderName::new("unnamed").unwrap(),
        home.path().join("unnamed"),
    )
    .unnamed_conversations()
    .reporting(Capability::Submit)
    .reporting(Capability::Cancel);
    let _daemon = home.start(vec![Arc::new(plain), Arc::new(unnamed)]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let plain = home.session(&client, &root, "plain").await;
    let unnamed = home.session(&client, &root, "unnamed").await;
    for (command, needed) in [
        (send(&plain.id, "hello"), "submit"),
        (cancel(&plain.id), "cancel"),
        (send(&unnamed.id, "hello"), "conversation"),
        (cancel(&unnamed.id), "conversation"),
    ] {
        let error = refused(client.request(command, GENEROUS).await);
        assert_eq!(error.code, ErrorCode::Refused, "{error:?}");
        assert!(error.message.contains(needed), "{error:?}");
    }
    let agent = cockpit_client::Connection::open(&home.options().agent_socket_path(), GENEROUS)
        .await
        .unwrap();
    for command in [send(&plain.id, "hello"), cancel(&plain.id)] {
        assert_eq!(
            refused(agent.request(command, GENEROUS).await).code,
            ErrorCode::NotAuthorized
        );
    }
}

#[tokio::test]
async fn empty_messages_and_ended_sessions_do_not_reach_the_provider() {
    let home = Home::new();
    let provider = Arc::new(Agent::new(&home, Transport::Direct));
    let _daemon = home.start(vec![provider.clone()]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;
    home.waiting(&session.id).await;
    for message in ["", " \n ", "nul\0byte"] {
        assert_eq!(
            refused(client.request(send(&session.id, message), GENEROUS).await).code,
            ErrorCode::InvalidArgument
        );
    }
    client
        .request(
            Command::SessionEnd {
                session: session.id.clone(),
            },
            GENEROUS,
        )
        .await
        .unwrap();
    for command in [send(&session.id, "hello"), cancel(&session.id)] {
        assert_eq!(
            refused(client.request(command, GENEROUS).await).code,
            ErrorCode::Conflict
        );
    }
    assert!(provider.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn an_open_dialog_blocks_delivery_but_an_owner_can_answer_the_agents_question() {
    let home = Home::new();
    let provider = Arc::new(Agent::new(&home, Transport::Direct));
    let _daemon = home.start(vec![provider.clone()]).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;
    home.waiting(&session.id).await;
    let launch = FakeProvider::launches(&home.path().join("launches")).remove(0);
    let peer = cockpit_agent_channel::ancestry(launch.pid as u32);
    for source in [
        cockpit_sessions::QuestionSource::Dialog,
        cockpit_sessions::QuestionSource::Agent,
    ] {
        let database = home.database.lock().unwrap().clone().unwrap();
        let write = database.write().await.unwrap();
        cockpit_sessions::report(
            &write,
            &peer,
            None,
            cockpit_sessions::Report {
                state: Some(cockpit_sessions::AgentState::Waiting),
                needs_input: (source == cockpit_sessions::QuestionSource::Agent)
                    .then(|| "which branch?".into()),
                dialog: (source == cockpit_sessions::QuestionSource::Dialog)
                    .then(|| "allow tool?".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        write.commit().await.unwrap();
        let answer = client.request(send(&session.id, "main"), GENEROUS).await;
        if source == cockpit_sessions::QuestionSource::Dialog {
            assert_eq!(refused(answer).code, ErrorCode::Refused);
            assert!(provider.calls.lock().unwrap().is_empty());
        } else {
            answer.unwrap();
        }
    }
    assert_eq!(
        *provider.calls.lock().unwrap(),
        vec![("conversation-1".into(), "main".into())]
    );
}

struct BlockedAgent {
    fake: FakeProvider,
    entered: tokio::sync::Notify,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}
impl Provider for BlockedAgent {
    fn name(&self) -> &ProviderName {
        self.fake.name()
    }
    fn capabilities(&self) -> Capabilities {
        self.fake.capabilities()
    }
    fn launch(&self, request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        self.fake.launch(request)
    }
    fn deliver(
        &self,
        _action: &Action<'_>,
        _target: &DeliveryTarget<'_>,
        _conversation: &ConversationId,
    ) -> Result<Delivered, ProviderError> {
        self.entered.notify_one();
        self.release.lock().unwrap().recv_timeout(GENEROUS).unwrap();
        Ok(Delivered::StartedTurn)
    }
}

#[tokio::test]
async fn a_timed_out_delivery_keeps_the_session_locked_until_it_actually_returns() {
    let home = Home::new();
    let (release, receiver) = std::sync::mpsc::channel();
    let provider = Arc::new(BlockedAgent {
        fake: FakeProvider::new(
            ProviderName::new("fake").unwrap(),
            home.path().join("launches"),
        )
        .reporting(Capability::Submit)
        .reporting(Capability::Cancel),
        entered: tokio::sync::Notify::new(),
        release: Mutex::new(receiver),
    });
    let mut wiring = common::wiring_in(home.path(), vec![provider.clone()]);
    wiring.sessions.step_timeout = std::time::Duration::from_secs(1);
    let _daemon = home.start_wired(wiring).await;
    let client = home.connect().await;
    let root = home.project(&client).await;
    let session = home.session(&client, &root, "fake").await;
    home.waiting(&session.id).await;
    let command = send(&session.id, "hello");
    let pending = client.request(command, GENEROUS);
    tokio::pin!(pending);
    tokio::select! {
        _ = provider.entered.notified() => {}
        reply = &mut pending => panic!("delivery never entered: {reply:?}"),
    }
    let error = refused(pending.await);
    assert!(error.message.contains("not confirmed"), "{error:?}");
    assert_eq!(
        refused(client.request(cancel(&session.id), GENEROUS).await).code,
        ErrorCode::Busy
    );
    release.send(()).unwrap();
}
