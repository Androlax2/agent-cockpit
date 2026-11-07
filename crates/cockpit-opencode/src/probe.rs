//! The runtime probe of an OpenCode version not on `tested-versions.txt` (#732).

use std::path::Path;
use std::time::Duration;

use cockpit_provider_api::{
    Capability, Missing, Probe, ProbeError, command_output, documents_flag, names_missing_from_file,
};

use crate::{LAUNCH_FLAGS, OPENCODE_VARIABLES};

/// How long `opencode --help` may take.
const HELP_DEADLINE: Duration = Duration::from_secs(5);

/// What the OpenCode at `program`, running `version`, lacks of what the adapter relies on:
/// each launch flag in `opencode --help` and each environment variable the adapter sets in the
/// program itself. Every one of them is used by every launch, so a missing one takes away every
/// capability. An error when OpenCode cannot be asked or read, which says nothing about the
/// version.
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
    for name in names_missing_from_file(program, &OPENCODE_VARIABLES)? {
        missing.push(Missing {
            requirement: format!("`{name}` in the opencode program"),
            blocks: Capability::ALL.to_vec(),
        });
    }
    Ok(Probe::new(version, missing))
}
