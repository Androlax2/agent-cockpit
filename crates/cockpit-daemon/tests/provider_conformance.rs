//! Shared refusal and provenance checks against the concrete provider adapters.

use std::os::unix::net::UnixStream;
use std::path::Path;

use cockpit_claude::ClaudeCode;
use cockpit_codex::Codex;
use cockpit_opencode::OpenCode;
use cockpit_provider_api::{
    Action, Availability, Capability, ConversationId, Delivered, DeliveryTarget, Provenance,
    Provider, ProviderError, SessionSockets,
};

fn claude() -> ClaudeCode {
    ClaudeCode::new(
        "/unused/claude".into(),
        None,
        Some(cockpit_claude::Bridge {
            hook_program: "/unused/cockpit-claude-hook".into(),
            cockpit_program: "/unused/cockpit".into(),
        }),
    )
}

fn codex() -> Codex {
    Codex::new(
        "/unused/codex".into(),
        None,
        Some(cockpit_codex::Bridge {
            session_program: "/unused/cockpit-codex-session".into(),
            hook_program: "/unused/cockpit-codex-hook".into(),
            cockpit_program: "/unused/cockpit".into(),
        }),
    )
}

struct NoSockets;

impl SessionSockets for NoSockets {
    fn connect(&self, socket: &Path) -> Result<UnixStream, ProviderError> {
        panic!(
            "unsupported operation tried to connect to {}",
            socket.display()
        )
    }
}

fn target() -> DeliveryTarget<'static> {
    DeliveryTarget {
        launch: None,
        session_dir: Path::new("/unused"),
        sockets: &NoSockets,
    }
}

#[test]
fn an_unknown_version_never_reports_tested_capabilities() {
    let opencode = OpenCode::new(
        "/unused/opencode".into(),
        None,
        Some(cockpit_opencode::Bridge {
            plugin: "/unused/cockpit-opencode-plugin.js".into(),
            bridge_program: "/unused/cockpit-opencode-bridge".into(),
            cockpit_program: "/unused/cockpit".into(),
        }),
    );
    let providers: [&dyn Provider; 3] = [&claude(), &codex(), &opencode];

    for provider in providers {
        let capabilities = provider.capabilities();
        assert!(
            capabilities.reported().next().is_some(),
            "{}",
            provider.name()
        );
        for (capability, provenance) in capabilities.reported() {
            assert_eq!(
                provenance,
                &Provenance::Discovered,
                "{} {capability:?}",
                provider.name()
            );
            assert_eq!(
                capabilities.availability(capability),
                Availability::Unverified,
                "{} {capability:?}",
                provider.name()
            );
        }
    }
}

#[test]
fn providers_without_commands_refuse_discovery() {
    let providers: [&dyn Provider; 2] = [&claude(), &codex()];

    for provider in providers {
        assert_eq!(
            provider.commands(&target()).unwrap_err(),
            ProviderError::unsupported(Capability::Commands),
            "{}",
            provider.name()
        );
    }
}

#[test]
fn providers_without_commands_refuse_invocation() {
    let providers: [&dyn Provider; 2] = [&claude(), &codex()];
    let conversation = ConversationId::new("conversation-1").unwrap();

    for provider in providers {
        assert_eq!(
            provider
                .invoke_command(&target(), &conversation, "review", "")
                .unwrap_err(),
            ProviderError::unsupported(Capability::Commands),
            "{}",
            provider.name()
        );
    }
}

#[test]
fn claude_direct_delivery_explicitly_requires_terminal_fallback() {
    let provider = claude();
    let conversation = ConversationId::new("conversation-1").unwrap();

    for action in [Action::Submit("hello"), Action::Cancel] {
        assert_eq!(
            provider.deliver(&action, &target(), &conversation).unwrap(),
            Delivered::Unsupported,
            "{action:?}"
        );
    }
}

#[test]
fn the_shipped_adapters_sandbox_declarations_are_accepted_together() {
    let opencode = OpenCode::new(
        "/unused/opencode".into(),
        None,
        Some(cockpit_opencode::Bridge {
            plugin: "/unused/cockpit-opencode-plugin.js".into(),
            bridge_program: "/unused/cockpit-opencode-bridge".into(),
            cockpit_program: "/unused/cockpit".into(),
        }),
    );
    let providers: [std::sync::Arc<dyn Provider>; 3] = [
        std::sync::Arc::new(claude()),
        std::sync::Arc::new(codex()),
        std::sync::Arc::new(opencode),
    ];

    assert_eq!(cockpit_sessions::refused_declarations(&providers), []);
}
