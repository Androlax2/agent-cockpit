//! What the contract decides for every provider: the capabilities a launch needs, and the
//! refusal a provider gives for an action it does not report.

use std::path::Path;

use cockpit_provider_api::{
    Action, Activity, Capabilities, Capability, Conversation, ConversationId, DeliveryTarget,
    LaunchCommand, LaunchRequest, Provenance, Provider, ProviderError, ProviderName,
    SessionSockets,
};

struct LaunchOnly(ProviderName);

#[test]
fn a_provider_without_a_sign_in_probe_reports_unknown_with_a_next_step() {
    let provider = LaunchOnly(ProviderName::new("plain").unwrap());
    let sign_in = provider.sign_in();
    assert_eq!(sign_in.provenance, Provenance::Discovered);
    let cockpit_provider_api::SignInStatus::Unknown { reason, next } = sign_in.status else {
        panic!("an unimplemented probe must not claim a sign-in status");
    };
    assert!(!reason.is_empty());
    assert!(next.contains("plain"));
}

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
        session_dir: Path::new("/run/cockpit/sessions/s-1"),
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
fn a_provider_without_delivery_refuses_each_action_naming_its_capability() {
    let provider = LaunchOnly(ProviderName::new("plain").unwrap());
    let conversation = ConversationId::new("c-1").unwrap();

    for (action, capability) in [
        (Action::Submit("hello"), "submit"),
        (Action::Cancel, "cancel"),
        (Action::Compact, "compact"),
    ] {
        let error = provider
            .deliver(
                &action,
                &DeliveryTarget {
                    launch: None,
                    session_dir: Path::new("/run/cockpit/sessions/s-1"),
                    sockets: &NoSockets,
                },
                &conversation,
            )
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("it does not report the `{capability}` capability")
        );
    }
}

#[test]
fn a_provider_without_commands_refuses_discovery_and_invocation() {
    let provider = LaunchOnly(ProviderName::new("plain").unwrap());
    let target = DeliveryTarget {
        launch: None,
        session_dir: Path::new("/unused"),
        sockets: &NoSockets,
    };
    let expected = ProviderError::unsupported(Capability::Commands);
    assert_eq!(provider.commands(&target).unwrap_err(), expected);
    assert_eq!(
        provider
            .invoke_command(&target, &ConversationId::new("c-1").unwrap(), "review", "")
            .unwrap_err(),
        expected
    );
}

#[test]
fn a_provider_without_screen_reading_reads_nothing() {
    let provider = LaunchOnly(ProviderName::new("plain").unwrap());

    assert_eq!(provider.read_screen("❯ "), None);
    assert_eq!(provider.version(), None);
}

#[test]
fn a_request_is_refused_naming_the_first_capability_the_provider_lacks() {
    let launch = LaunchRequest {
        effort: Some("high"),
        remote_control: true,
        ..request(&Conversation::New)
    };
    let capabilities = Capabilities::none().with(Capability::Effort, Provenance::Discovered);

    let error = launch.require(&capabilities).unwrap_err();

    assert_eq!(
        error.to_string(),
        ProviderError::unsupported(Capability::RemoteControl).to_string()
    );
}

#[test]
fn a_request_whose_capabilities_are_all_reported_is_accepted() {
    let launch = LaunchRequest {
        effort: Some("high"),
        ..request(&Conversation::New)
    };
    let capabilities = Capabilities::none().with(Capability::Effort, Provenance::Discovered);

    assert!(launch.require(&capabilities).is_ok());
}

/// Opens nothing: a provider without delivery never asks.
struct NoSockets;

impl SessionSockets for NoSockets {
    fn connect(&self, socket: &Path) -> Result<std::os::unix::net::UnixStream, ProviderError> {
        panic!("a provider without delivery opened {}", socket.display())
    }
}
