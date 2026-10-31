//! What the contract decides for every provider: the capabilities a launch needs, and the
//! refusal a provider gives for an action it does not report.

use std::path::Path;

use cockpit_provider_api::{
    Action, Activity, Capabilities, Capability, Conversation, ConversationId, LaunchCommand,
    LaunchRequest, Provider, ProviderError, ProviderName,
};

struct LaunchOnly(ProviderName);

impl Provider for LaunchOnly {
    fn name(&self) -> &ProviderName {
        &self.0
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities::none()
    }

    fn launch(&self, _request: &LaunchRequest<'_>) -> Result<LaunchCommand, ProviderError> {
        unreachable!("not launched here")
    }
}

fn request(conversation: &Conversation) -> LaunchRequest<'_> {
    LaunchRequest {
        working_dir: Path::new("/work"),
        conversation,
        first_message: None,
        effort: None,
        remote_control: false,
    }
}

#[test]
fn a_new_plain_launch_needs_no_capability() {
    assert_eq!(request(&Conversation::New).needed_capabilities(), []);
}

#[test]
fn a_resumed_launch_with_effort_and_remote_control_needs_each_capability() {
    let conversation = Conversation::Resume(ConversationId::new("c-1").unwrap());
    let launch = LaunchRequest {
        effort: Some("high"),
        remote_control: true,
        ..request(&conversation)
    };

    assert_eq!(
        launch.needed_capabilities(),
        [
            Capability::Resume,
            Capability::Effort,
            Capability::RemoteControl
        ]
    );
}

#[test]
fn a_provider_without_terminal_actions_refuses_each_naming_its_capability() {
    let provider = LaunchOnly(ProviderName::new("plain").unwrap());

    for (action, capability) in [
        (Action::Submit("hello"), "submit"),
        (Action::Cancel, "cancel"),
        (Action::Compact, "compact"),
    ] {
        let error = provider
            .input(&action, Some(Activity::Waiting))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("it does not report the `{capability}` capability")
        );
    }
}

#[test]
fn a_provider_without_screen_reading_reads_nothing() {
    let provider = LaunchOnly(ProviderName::new("plain").unwrap());

    assert_eq!(provider.read_screen("❯ "), None);
    assert_eq!(provider.version(), None);
}
