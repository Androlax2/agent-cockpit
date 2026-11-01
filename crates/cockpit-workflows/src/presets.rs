//! The product's workflow presets (report 06). A preset is identified by its name and
//! version: a run keeps the version it started with, so changing a preset's steps means a new
//! version beside the old one, never an edit of a version runs already follow.

use cockpit_provider_api::Capability;
use cockpit_tasks::Kind;

use crate::Refusal;

/// Who carries a step out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doer {
    /// A session begins it, records checkpoints and completes it with a handoff. Scheduling
    /// places ready agent steps; workflows never do.
    Agent,
    /// The owner completes it, as an approval of what came before.
    Owner,
}

impl Doer {
    pub fn as_str(self) -> &'static str {
        match self {
            Doer::Agent => "agent",
            Doer::Owner => "owner",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct StepDefinition {
    pub name: &'static str,
    /// What the step produces, in one line, as the task packet shows it.
    pub purpose: &'static str,
    pub doer: Doer,
    /// What the provider of the session beginning the step must report
    /// ([`crate::check_capabilities`]).
    pub required_capabilities: &'static [Capability],
    /// Evidence the step needs before it completes, by name. `cockpit-verification` decides
    /// whether a requirement passed against the exact result; the caller passes the names it
    /// passed. A handoff's prose never satisfies one.
    pub requirements: &'static [&'static str],
}

#[derive(Debug, PartialEq, Eq)]
pub struct Preset {
    pub name: &'static str,
    pub version: u32,
    pub summary: &'static str,
    pub steps: &'static [StepDefinition],
}

impl Preset {
    /// The step at `position`, counted from 1 as runs store it.
    pub fn step(&self, position: u32) -> Option<&'static StepDefinition> {
        let index = usize::try_from(position.checked_sub(1)?).ok()?;
        let steps: &'static [StepDefinition] = self.steps;
        steps.get(index)
    }

    /// The step positions, counted from 1.
    pub fn positions(&self) -> impl Iterator<Item = u32> {
        (1..).take(self.steps.len())
    }
}

/// An agent step learns that a turn ended, and how, from the provider's events: a prompt it
/// accepted is not work it completed (report 08).
const OBSERVED: &[Capability] = &[Capability::Events, Capability::Outcome];

pub const RESEARCH: Preset = Preset {
    name: "research",
    version: 1,
    summary: "investigate, synthesize, deliver",
    steps: &[
        StepDefinition {
            name: "investigate",
            purpose: "gather the sources and facts the question needs",
            doer: Doer::Agent,
            required_capabilities: OBSERVED,
            requirements: &[],
        },
        StepDefinition {
            name: "synthesize",
            purpose: "turn the findings into an answer with its sources",
            doer: Doer::Agent,
            required_capabilities: OBSERVED,
            requirements: &[],
        },
        StepDefinition {
            name: "deliver",
            purpose: "hand the answer over where the task asks for it",
            doer: Doer::Agent,
            required_capabilities: OBSERVED,
            requirements: &[],
        },
    ],
};

pub const CODE_CHANGE: Preset = Preset {
    name: "code-change",
    version: 1,
    summary: "implement, required checks and review, deliver",
    steps: &[
        StepDefinition {
            name: "implement",
            purpose: "make the change the task asks for",
            doer: Doer::Agent,
            required_capabilities: OBSERVED,
            requirements: &[],
        },
        StepDefinition {
            name: "check",
            purpose: "run the project's required checks and review on the exact result",
            doer: Doer::Agent,
            required_capabilities: OBSERVED,
            requirements: &["checks", "review"],
        },
        StepDefinition {
            name: "deliver",
            purpose: "hand the checked result over where the task asks for it",
            doer: Doer::Agent,
            required_capabilities: OBSERVED,
            requirements: &[],
        },
    ],
};

pub const DESIGN_LED: Preset = Preset {
    name: "design-led",
    version: 1,
    summary: "approved design, implementation, required verification",
    steps: &[
        StepDefinition {
            name: "design-approval",
            purpose: "the owner approves the design the implementation follows",
            doer: Doer::Owner,
            required_capabilities: &[],
            requirements: &[],
        },
        StepDefinition {
            name: "implement",
            purpose: "build what the approved design shows",
            doer: Doer::Agent,
            required_capabilities: OBSERVED,
            requirements: &[],
        },
        StepDefinition {
            name: "verify",
            purpose: "prove the result matches the design and passes the required checks",
            doer: Doer::Agent,
            required_capabilities: OBSERVED,
            requirements: &["checks", "design-match"],
        },
    ],
};

/// Every version of every preset. A version is never removed while a stored run may follow
/// it: reading such a run fails loud instead of guessing its steps.
pub const PRESETS: &[&Preset] = &[&RESEARCH, &CODE_CHANGE, &DESIGN_LED];

/// The newest version of the preset named `name`.
pub fn preset(name: &str) -> Option<&'static Preset> {
    PRESETS
        .iter()
        .copied()
        .filter(|preset| preset.name == name)
        .max_by_key(|preset| preset.version)
}

/// The exact version a stored run follows.
pub fn preset_version(name: &str, version: u32) -> Option<&'static Preset> {
    PRESETS
        .iter()
        .copied()
        .find(|preset| preset.name == name && preset.version == version)
}

/// The newest version of every preset, in [`PRESETS`] order.
pub fn presets() -> Vec<&'static Preset> {
    let mut newest: Vec<&'static Preset> = Vec::new();
    for candidate in PRESETS.iter().copied() {
        if newest.iter().any(|preset| preset.name == candidate.name) {
            continue;
        }
        newest.extend(preset(candidate.name));
    }
    newest
}

/// The preset a task follows: the one asked for (the command, else the project's
/// `workflow.preset`), else `research` for a research item and `code-change` for any other.
/// A name no preset has is refused, never replaced by the default.
pub fn select_preset(asked: Option<&str>, kind: Kind) -> Result<&'static Preset, Refusal> {
    match asked {
        Some(name) => preset(name).ok_or_else(|| Refusal::UnknownPreset {
            name: name.to_owned(),
            known: presets().iter().map(|preset| preset.name).collect(),
        }),
        None if kind == Kind::Research => Ok(&RESEARCH),
        None => Ok(&CODE_CHANGE),
    }
}
