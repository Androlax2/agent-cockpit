//! The runtime probe of a Qwen Code version not on `tested-versions.txt` (#732).

use std::path::Path;
use std::time::Duration;

use cockpit_provider_api::{
    Capability, Missing, Probe, ProbeError, command_output, documents_flag,
};

use crate::LAUNCH_FLAGS;

/// How long `qwen --help` may take.
const HELP_DEADLINE: Duration = Duration::from_secs(10);

/// What the Qwen Code at `program`, running `version`, lacks of what the adapter relies on:
/// each launch flag in `qwen --help`. Every launch uses them, so a missing one takes away every
/// capability. [`crate::SYSTEM_DEFAULTS_VARIABLE`] is not looked for: the program on `PATH` is
/// a wrapper or an entry script, and the bundle splits Qwen Code into chunks beside it; the
/// live conformance test reads them. An error when Qwen Code cannot be asked, which says
/// nothing about the version.
pub fn probe(program: &Path, version: &str) -> Result<Probe, ProbeError> {
    let help = command_output(program, &["--help"], HELP_DEADLINE)?;
    let missing = LAUNCH_FLAGS
        .into_iter()
        .filter(|flag| !documents_flag(&help, flag))
        .map(|flag| Missing {
            requirement: format!("flag {flag} in `qwen --help`"),
            blocks: Capability::ALL.to_vec(),
        })
        .collect();
    Ok(Probe::new(version, missing))
}
