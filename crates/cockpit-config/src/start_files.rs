//! Files an agent reads only when it starts: after one changes, sessions started before it run
//! on the old version until they restart. The list is declared (product defaults plus the
//! owner's `[[start_files]]`), and change detection compares a snapshot taken at launch.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::SystemTime;

/// Provider names a start file may list. The adapter crates (report 02) become the source of
/// this list once they exist; until then a typo fails here instead of never matching.
pub const KNOWN_PROVIDERS: [&str; 3] = ["claude", "codex", "opencode"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartFile {
    /// Unique name shown to the owner, e.g. "CLAUDE.md".
    pub label: String,
    pub path: PathBuf,
    /// The providers whose sessions read this file at launch.
    pub providers: Vec<String>,
}

impl StartFile {
    pub fn is_read_by(&self, provider: &str) -> bool {
        self.providers.iter().any(|reader| reader == provider)
    }
}

/// Provider-owned files every user of that provider has. Sandbox launchers, learned profiles
/// and personal hooks are installation-specific: owners declare them in their config.
pub(crate) const DEFAULT_START_FILES: [(&str, &str, &[&str]); 5] = [
    ("CLAUDE.md", ".claude/CLAUDE.md", &["claude", "opencode"]),
    ("settings", ".claude/settings.json", &["claude"]),
    (
        "plugins",
        ".claude/plugins/installed_plugins.json",
        &["claude"],
    ),
    (
        "opencode config",
        ".config/opencode/opencode.jsonc",
        &["opencode"],
    ),
    ("codex hooks", ".codex/hooks.json", &["codex"]),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileState {
    Missing,
    Present {
        modified: SystemTime,
        len: u64,
    },
    /// Metadata could not be read for a reason other than absence (permissions, I/O).
    Unreadable {
        error: String,
    },
}

impl FileState {
    pub fn read(path: &std::path::Path) -> Self {
        match std::fs::metadata(path) {
            Ok(metadata) => match metadata.modified() {
                Ok(modified) => Self::Present {
                    modified,
                    len: metadata.len(),
                },
                Err(error) => Self::Unreadable {
                    error: error.to_string(),
                },
            },
            Err(error) if error.kind() == ErrorKind::NotFound => Self::Missing,
            Err(error) => Self::Unreadable {
                error: error.to_string(),
            },
        }
    }
}

/// The state of each start file a session's provider reads, taken when the session launches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartFileSnapshot {
    pub provider: String,
    pub states: Vec<(String, FileState)>,
}

impl StartFileSnapshot {
    pub fn capture(start_files: &[StartFile], provider: &str) -> Self {
        Self {
            provider: provider.to_string(),
            states: start_files
                .iter()
                .filter(|start_file| start_file.is_read_by(provider))
                .map(|start_file| (start_file.label.clone(), FileState::read(&start_file.path)))
                .collect(),
        }
    }

    /// Labels of the files this session's provider reads whose state differs from the
    /// snapshot. Any difference counts, so a file replaced by an older copy is reported too.
    /// A file declared after the snapshot was taken is reported: the session cannot have it.
    pub fn changed<'files>(&self, start_files: &'files [StartFile]) -> Vec<&'files str> {
        start_files
            .iter()
            .filter(|start_file| start_file.is_read_by(&self.provider))
            .filter(|start_file| {
                let recorded = self
                    .states
                    .iter()
                    .find(|(label, _)| *label == start_file.label)
                    .map(|(_, state)| state);
                recorded != Some(&FileState::read(&start_file.path))
            })
            .map(|start_file| start_file.label.as_str())
            .collect()
    }
}
