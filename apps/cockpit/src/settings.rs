//! Theo's on/off choices, kept by `claude-sessions setting` in settings.json. A setting missing
//! from the file is on: what the setup did before it had settings.

use crate::paths;
use crate::util::read_json;

/// Every setting the S menu shows: its name in settings.json and claude-sessions (KNOWN_SETTINGS
/// there), and what it does.
pub const SETTINGS: [(&str, &str); 6] = [
    ("phone-on-lock", "Phone on lock"),
    ("todo-queue", "Todo queue (Q)"),
    ("error-handoff", "Errors to dotfiles"),
    ("freeze-while-gaming", "Freeze while gaming"),
    ("idle-lock", "Lock workspace 6 idle"),
    ("daily-digest", "Daily digest"),
];

pub fn is_on(name: &str) -> bool {
    read_json(&paths::settings()).and_then(|settings| settings.get(name)?.as_bool()).unwrap_or(true)
}
