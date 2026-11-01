//! The fake provider (feature `fake`) that `cockpit-sessions`' tests launch: the baseline
//! contract only, a shell agent that logs each launch.

use cockpit_provider_api::fake::FakeProvider;
use cockpit_provider_api::{
    Action, Activity, Capability, Conversation, ConversationId, Key, LaunchRequest, Provider,
    ProviderName, TerminalInput,
};

fn request(conversation: &Conversation) -> LaunchRequest<'_> {
    LaunchRequest {
        working_dir: std::path::Path::new("/"),
        conversation,
        first_message: Some("hello"),
        effort: None,
        remote_control: false,
    }
}

fn fake(dir: &tempfile::TempDir) -> FakeProvider {
    FakeProvider::new(
        ProviderName::new("fake").unwrap(),
        dir.path().join("agent.log"),
    )
}

#[test]
fn the_fake_reports_resume_and_nothing_else() {
    let dir = tempfile::tempdir().unwrap();
    let capabilities = fake(&dir).capabilities();

    let reported: Vec<Capability> = capabilities.reported().map(|(c, _)| c).collect();
    assert_eq!(reported, [Capability::Resume]);
}

#[test]
fn the_fake_without_resume_refuses_a_resumed_launch() {
    let dir = tempfile::tempdir().unwrap();
    let provider = fake(&dir).without_resume();
    let conversation = Conversation::Resume(ConversationId::new("conversation-1").unwrap());

    assert!(provider.launch(&request(&conversation)).is_err());
}

#[test]
fn a_new_conversation_is_named_and_counted_after_the_logged_launches() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("agent.log"),
        "s1 new conversation-1 x pid=1 token=t\n",
    )
    .unwrap();

    let command = fake(&dir).launch(&request(&Conversation::New)).unwrap();

    assert_eq!(
        command.conversation,
        Some(ConversationId::new("conversation-2").unwrap())
    );
}

#[test]
fn an_unnamed_fake_launches_without_naming_the_conversation() {
    let dir = tempfile::tempdir().unwrap();
    let command = fake(&dir)
        .unnamed_conversations()
        .launch(&request(&Conversation::New))
        .unwrap();

    assert_eq!(command.conversation, None);
}

#[test]
fn the_fake_agent_logs_its_launch_and_the_log_reads_back() {
    let dir = tempfile::tempdir().unwrap();
    let provider = fake(&dir);
    let command = provider.launch(&request(&Conversation::New)).unwrap();
    let mut agent = std::process::Command::new(&command.program);
    agent
        .args(&command.arguments)
        .env_clear()
        .envs(command.environment.iter().cloned())
        .env("COCKPIT_SESSION", "s000000000001")
        .env("COCKPIT_LAUNCH_TOKEN", "token-1")
        // The agent sleeps after logging; the test needs only the log line.
        .env("FAKE_AGENT_SLEEP", "0");
    assert!(agent.status().unwrap().success());

    let launches = FakeProvider::launches(&dir.path().join("agent.log"));
    assert_eq!(launches.len(), 1);
    let launch = &launches[0];
    assert_eq!(
        (
            launch.session.as_str(),
            launch.mode.as_str(),
            launch.conversation.as_str(),
            launch.message.as_str(),
            launch.token.as_str()
        ),
        ("s000000000001", "new", "conversation-1", "hello", "token-1")
    );
}

#[test]
fn a_fake_reading_messages_types_one_while_its_agent_waits_and_refuses_one_under_a_dialog() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake(&dir).reading_messages(dir.path().join("messages.log"));

    assert!(fake.capabilities().supports(Capability::Submit));
    assert_eq!(
        fake.input(&Action::Submit("do item 3"), Some(Activity::Waiting))
            .unwrap(),
        vec![
            TerminalInput::Text("do item 3".to_owned()),
            TerminalInput::Key(Key::Enter)
        ]
    );
    assert!(
        fake.input(&Action::Submit("do item 3"), Some(Activity::NeedsYou))
            .is_err()
    );
}

#[test]
fn the_fake_without_submit_refuses_a_message() {
    let dir = tempfile::tempdir().unwrap();

    let refused = fake(&dir).input(&Action::Submit("do item 3"), Some(Activity::Waiting));

    assert_eq!(
        refused.unwrap_err().reason,
        "it does not report the `submit` capability"
    );
}
