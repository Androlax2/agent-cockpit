//! The runtime probe of a Claude Code version not on `tested-versions.txt` (#732).

use std::path::Path;
use std::time::Duration;

use cockpit_provider_api::{
    Capability, Missing, Probe, ProbeError, command_output, documents_flag, names_missing_from_file,
};

use crate::{LAUNCH_ENVIRONMENT, LAUNCH_FLAGS, hooks};

/// How long `claude --help` may take.
const HELP_DEADLINE: Duration = Duration::from_secs(5);

/// The capabilities that cannot be offered without `flag`. A flag every launch passes (or one
/// this match does not know) takes everything away, since Claude Code refuses an unknown
/// option.
fn flag_blocks(flag: &str) -> Vec<Capability> {
    match flag {
        "--resume" => vec![Capability::Resume],
        "--effort" => vec![Capability::Effort],
        "--model" => vec![Capability::Model],
        "--remote-control" => vec![Capability::RemoteControl],
        "--autocompact" => vec![Capability::CompactionThreshold],
        _ => Capability::ALL.to_vec(),
    }
}

/// What the Claude Code at `program`, running `version`, lacks of what the adapter relies on:
/// each launch flag in `claude --help`, each launch environment name and each
/// hook event in the program itself. Reads the program once, which takes seconds; an error
/// when `--help` or the program cannot be read, which says nothing about the version.
pub fn probe(program: &Path, version: &str) -> Result<Probe, ProbeError> {
    let help = command_output(program, &["--help"], HELP_DEADLINE)?;
    let mut missing: Vec<Missing> = Vec::new();
    for flag in LAUNCH_FLAGS {
        if !documents_flag(&help, flag) {
            missing.push(Missing {
                requirement: format!("flag {flag} in `claude --help`"),
                blocks: flag_blocks(flag),
            });
        }
    }
    let bridged = vec![Capability::Events, Capability::Outcome, Capability::Inbox];
    let requirements: Vec<(&str, Vec<Capability>)> = LAUNCH_ENVIRONMENT
        .iter()
        .map(|(name, _)| (*name, Capability::ALL.to_vec()))
        .chain(
            hooks::HOOK_EVENTS
                .iter()
                .map(|event| (*event, bridged.clone())),
        )
        .collect();
    let names: Vec<&str> = requirements.iter().map(|(name, _)| *name).collect();
    for lost in names_missing_from_file(program, &names)? {
        let blocks = requirements
            .iter()
            .find_map(|(name, blocks)| (*name == lost).then(|| blocks.clone()))
            .unwrap_or_default();
        missing.push(Missing {
            requirement: format!("`{lost}` in the claude program"),
            blocks,
        });
    }
    Ok(Probe::new(version, missing))
}
