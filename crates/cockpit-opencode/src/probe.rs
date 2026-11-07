//! The runtime probe of an OpenCode version not on `tested-versions.txt` (#732).

use std::path::Path;
use std::time::Duration;

use cockpit_provider_api::{
    Capability, Missing, Probe, ProbeError, command_output, documents_flag, names_missing_from_file,
};

use crate::{COMPACTION_BUFFER_SETTING, LAUNCH_FLAGS, OPENCODE_VARIABLES};

/// How long `opencode --help` may take.
const HELP_DEADLINE: Duration = Duration::from_secs(5);

/// What the OpenCode at `program`, running `version`, lacks of what the adapter relies on:
/// each launch flag in `opencode --help`, each environment variable the adapter sets and the
/// compaction buffer setting's name in the program itself. The flags and variables are used
/// by every launch, so a missing one takes away every capability; the setting is used only by
/// a launch with a compaction threshold, so its loss takes away that capability alone (#783).
/// An error when OpenCode cannot be asked or read, which says nothing about the version.
pub fn probe(program: &Path, version: &str) -> Result<Probe, ProbeError> {
    let help = command_output(program, &["--help"], HELP_DEADLINE)?;
    let mut missing: Vec<Missing> = Vec::new();
    for flag in LAUNCH_FLAGS {
        if !documents_flag(&help, flag) {
            missing.push(Missing {
                requirement: format!("flag {flag} in `opencode --help`"),
                blocks: Capability::ALL.to_vec(),
            });
        }
    }
    let names: Vec<&str> = OPENCODE_VARIABLES
        .into_iter()
        .chain([COMPACTION_BUFFER_SETTING])
        .collect();
    for name in names_missing_from_file(program, &names)? {
        let (what, blocks) = if name == COMPACTION_BUFFER_SETTING {
            ("setting", vec![Capability::CompactionThreshold])
        } else {
            ("variable", Capability::ALL.to_vec())
        };
        missing.push(Missing {
            requirement: format!("{what} `{name}` in the opencode program"),
            blocks,
        });
    }
    Ok(Probe::new(version, missing))
}
