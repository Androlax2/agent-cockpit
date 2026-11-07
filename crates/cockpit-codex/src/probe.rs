//! The runtime probe of a Codex version not on `tested-versions.txt` (#732).

use std::path::Path;
use std::time::Duration;

use cockpit_provider_api::{
    Capability, Missing, Probe, ProbeError, command_output, documents_flag,
};

use crate::{BRIDGED_CAPABILITIES, HOOKS_FEATURE, LAUNCH_FLAGS, SERVER_FLAGS, TUI_FLAGS};

/// How long each Codex command the probe runs may take.
const COMMAND_DEADLINE: Duration = Duration::from_secs(5);

/// What the Codex at `program`, running `version`, lacks of what the adapter relies on: each
/// launch flag in `codex --help` and `codex resume --help`, each server flag in
/// `codex app-server --help`, and the hooks feature in `codex features list`. An error when
/// Codex cannot be asked, which says nothing about the version.
pub fn probe(program: &Path, version: &str) -> Result<Probe, ProbeError> {
    let mut missing: Vec<Missing> = Vec::new();
    for command in [&["--help"][..], &["resume", "--help"][..]] {
        let help = command_output(program, command, COMMAND_DEADLINE)?;
        for flag in LAUNCH_FLAGS.into_iter().chain(TUI_FLAGS) {
            if !documents_flag(&help, flag) {
                missing.push(Missing {
                    requirement: format!("flag {flag} in `codex {}`", command.join(" ")),
                    blocks: Capability::ALL.to_vec(),
                });
            }
        }
    }
    let server_help = command_output(program, &["app-server", "--help"], COMMAND_DEADLINE)?;
    for flag in SERVER_FLAGS {
        if !documents_flag(&server_help, flag) {
            missing.push(Missing {
                requirement: format!("flag {flag} in `codex app-server --help`"),
                blocks: Capability::ALL.to_vec(),
            });
        }
    }
    let features = command_output(program, &["features", "list"], COMMAND_DEADLINE)?;
    if !features
        .lines()
        .any(|line| line.split_whitespace().next() == Some(HOOKS_FEATURE))
    {
        missing.push(Missing {
            requirement: format!("feature {HOOKS_FEATURE} in `codex features list`"),
            blocks: BRIDGED_CAPABILITIES.to_vec(),
        });
    }
    Ok(Probe::new(version, missing))
}
