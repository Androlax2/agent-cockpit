//! The agents a session can run (Claude Code, opencode): one row each, read by the column, the
//! phone page and the daemon. claude-sessions has the same agents as files
//! (config/bin/.local/bin/claude-sessions-agents/<name>.sh), and a test keeps both lists equal.
//! Adding a CLI takes three pieces:
//! - a row here
//! - a file there
//! - a bridge that sends its states as session-state.sh payloads with CLAUDE_SESSIONS_AGENT=<name>
//!   (Claude's hooks, opencode's plugin)

use regex::Regex;
use std::sync::LazyLock;

/// What an agent may or may not do. The cockpit refuses a missing feature by name instead of
/// typing something into a screen where it means something else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// claude-sessions restarts it on another effort level.
    Effort,
    /// Open its native model variant picker without restarting.
    Variants,
    /// It bills the Console's API credits or the subscription.
    Billing,
    /// /compact.
    Compact,
    /// Claude's Remote Control (/rc).
    RemoteControl,
    /// The phone answers its yes/no dialogs with Enter and Escape.
    Dialogs,
    /// The phone interrupts its turn with Escape.
    Interrupt,
    /// The daemon reads its state from its screen (osd classify's patterns), over stale reports.
    ScreenStates,
}

pub struct Agent {
    /// claude-sessions' --agent name, and the agent field of its reports.
    pub name: &'static str,
    /// Shown in messages.
    pub label: &'static str,
    /// The card's badge; none for the default agent, which every card was before.
    pub badge: Option<&'static str>,
    /// Its tmux sessions' names before its first report; the longest matching prefix wins.
    pub tmux_prefix: &'static str,
    /// The ids it reports.
    pub id_pattern: &'static str,
    /// The name of the one process that keeps a session alive while it runs in its pane; None for
    /// Claude, whose process tree the daemon knows (claude_processes).
    pub process: Option<&'static str>,
    pub features: &'static [Feature],
    /// The tmux keys that stop its turn (Feature::Interrupt), in one send-keys.
    pub interrupt_keys: &'static [&'static str],
    /// The sessions::START_FILES it reads when it starts.
    pub start_files: &'static [&'static str],
}

impl Feature {
    /// Its name for the phone page, which shows a session's controls by its features.
    pub fn name(self) -> &'static str {
        match self {
            Feature::Effort => "effort",
            Feature::Variants => "variants",
            Feature::Billing => "billing",
            Feature::Compact => "compact",
            Feature::RemoteControl => "remote_control",
            Feature::Dialogs => "dialogs",
            Feature::Interrupt => "interrupt",
            Feature::ScreenStates => "screen_states",
        }
    }
}

impl Agent {
    pub fn can(&self, feature: Feature) -> bool {
        self.features.contains(&feature)
    }

    pub fn feature_names(&self) -> Vec<&'static str> {
        self.features.iter().map(|feature| feature.name()).collect()
    }
}

/// The agent of every session that does not say: all of them did before there was another.
pub const DEFAULT: &str = "claude";

pub static AGENTS: [Agent; 2] = [
    Agent {
        name: "claude",
        label: "Claude Code",
        badge: None,
        tmux_prefix: "claude-retry-",
        id_pattern: r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$",
        process: None,
        features: &[Feature::Effort, Feature::Billing, Feature::Compact, Feature::RemoteControl, Feature::Dialogs,
                    Feature::Interrupt, Feature::ScreenStates],
        interrupt_keys: &["Escape"],
        start_files: &["CLAUDE.md", "settings", "plugins", "ADHD mode", "session hook", "sandbox", "sandbox profile",
                       "launcher", "shared launcher"],
    },
    Agent {
        name: "opencode",
        label: "opencode",
        badge: Some("◇ opencode"),
        tmux_prefix: "claude-retry-opencode-",
        id_pattern: r"^ses_[0-9A-Za-z]{12,40}$",
        process: Some("opencode"),
        // Escape twice stops its turn (the first only says "esc again to interrupt"). Its permission
        // questions are not Claude's yes/no menu: no Dialogs.
        features: &[Feature::Interrupt, Feature::Variants],
        interrupt_keys: &["Escape", "Escape"],
        start_files: &["CLAUDE.md", "sandbox", "shared launcher", "opencode config", "opencode plugin", "opencode profile",
                       "opencode launcher"],
    },
];

static ID_PATTERNS: LazyLock<Vec<Regex>> =
    LazyLock::new(|| AGENTS.iter().map(|agent| Regex::new(agent.id_pattern).expect("an agent's id pattern")).collect());

pub fn names() -> Vec<&'static str> {
    AGENTS.iter().map(|agent| agent.name).collect()
}

pub fn find(name: &str) -> Option<&'static Agent> {
    AGENTS.iter().find(|agent| agent.name == name)
}

/// The agent named, or the default for a name no agent has (a report that did not say).
pub fn get(name: &str) -> &'static Agent {
    find(name).unwrap_or_else(|| find(DEFAULT).expect("the default agent has a row"))
}

/// Whether `id` is an id some agent reports.
pub fn is_session_id(id: &str) -> bool {
    ID_PATTERNS.iter().any(|pattern| pattern.is_match(id))
}

/// The agent whose reported ids `id` matches.
pub fn of_session_id(id: &str) -> Option<&'static Agent> {
    AGENTS.iter().zip(ID_PATTERNS.iter()).find(|(_, pattern)| pattern.is_match(id)).map(|(agent, _)| agent)
}

/// The agent of a tmux session not reported yet, by the longest prefix of its name.
pub fn of_tmux_session(name: &str) -> Option<&'static Agent> {
    AGENTS.iter().filter(|agent| name.starts_with(agent.tmux_prefix)).max_by_key(|agent| agent.tmux_prefix.len())
}
