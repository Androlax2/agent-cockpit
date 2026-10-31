//! What a provider can do and how Cockpit knows it (report 08).

use std::collections::BTreeMap;

use crate::ProviderError;

/// One thing a provider may be able to do. Callers key every behavior on these, never on the
/// provider's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Capability {
    /// Launch a conversation it started before, with its history.
    Resume,
    /// Type a message into a running conversation ([`crate::Action::Submit`]).
    Submit,
    /// Its in-sandbox bridge reports the agent's activity through the agent channel.
    Events,
    /// Interrupt the turn under way ([`crate::Action::Cancel`]).
    Cancel,
    /// Its turn-end events say how the turn ended ([`crate::TurnOutcome`]).
    Outcome,
    /// List the commands the harness offers. No contract method yet: it arrives with its
    /// first caller.
    Commands,
    /// Retrieve evidence of a turn (transcripts, tool results). No contract method yet.
    Evidence,
    /// Launch at a chosen reasoning effort ([`crate::LaunchRequest::effort`]).
    Effort,
    /// Report which billing (subscription or API credits) a session runs on. No contract
    /// method yet: account routing (report 18) brings it.
    Billing,
    /// Compact the conversation's context ([`crate::Action::Compact`]).
    Compact,
    /// Launch with the provider's own remote control on
    /// ([`crate::LaunchRequest::remote_control`]).
    RemoteControl,
    /// Read its activity from its terminal when no event says it
    /// ([`crate::Provider::read_screen`]); a fallback with recorded confidence.
    ScreenReading,
}

impl Capability {
    pub const ALL: [Capability; 12] = [
        Capability::Resume,
        Capability::Submit,
        Capability::Events,
        Capability::Cancel,
        Capability::Outcome,
        Capability::Commands,
        Capability::Evidence,
        Capability::Effort,
        Capability::Billing,
        Capability::Compact,
        Capability::RemoteControl,
        Capability::ScreenReading,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Capability::Resume => "resume",
            Capability::Submit => "submit",
            Capability::Events => "events",
            Capability::Cancel => "cancel",
            Capability::Outcome => "outcome",
            Capability::Commands => "commands",
            Capability::Evidence => "evidence",
            Capability::Effort => "effort",
            Capability::Billing => "billing",
            Capability::Compact => "compact",
            Capability::RemoteControl => "remote-control",
            Capability::ScreenReading => "screen-reading",
        }
    }
}

/// How the provider knows it can do something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// The adapter's conformance tests pass against this exact harness version, the one
    /// running.
    Tested { version: String },
    /// Documented or detected, but not tested against the version running.
    Discovered,
}

/// What the owner's capability report shows for one capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    Available { version: String },
    Unverified,
    Unavailable,
}

/// The capabilities a provider reports. One not reported is unavailable.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Capabilities {
    reported: BTreeMap<Capability, Provenance>,
}

impl Capabilities {
    pub fn none() -> Capabilities {
        Capabilities::default()
    }

    /// Reports `capability`, replacing what was reported for it before.
    pub fn with(mut self, capability: Capability, provenance: Provenance) -> Capabilities {
        self.reported.insert(capability, provenance);
        self
    }

    /// Whether the provider reports `capability`, tested or not.
    pub fn supports(&self, capability: Capability) -> bool {
        self.provenance(capability).is_some()
    }

    pub fn provenance(&self, capability: Capability) -> Option<&Provenance> {
        self.reported.get(&capability)
    }

    pub fn availability(&self, capability: Capability) -> Availability {
        match self.provenance(capability) {
            Some(Provenance::Tested { version }) => Availability::Available {
                version: version.clone(),
            },
            Some(Provenance::Discovered) => Availability::Unverified,
            None => Availability::Unavailable,
        }
    }

    /// The refusal for an operation that needs `capability`, when it is not reported.
    pub fn require(&self, capability: Capability) -> Result<(), ProviderError> {
        if self.supports(capability) {
            Ok(())
        } else {
            Err(ProviderError::unsupported(capability))
        }
    }

    /// Every reported capability with its provenance, in [`Capability`] order.
    pub fn reported(&self) -> impl Iterator<Item = (Capability, &Provenance)> {
        self.reported
            .iter()
            .map(|(capability, provenance)| (*capability, provenance))
    }
}
