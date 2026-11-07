//! The runtime probe of a Codex version not on `tested-versions.txt` (#732).

use std::path::Path;
use std::time::Duration;

use cockpit_provider_api::{
    Capability, Missing, Probe, ProbeError, command_output, documents_flag,
};

use crate::{
    BRIDGED_CAPABILITIES, COMPACTION_THRESHOLD_KEY, COMPACTION_THRESHOLD_PROBE_VALUE, CONFIG_FLAG,
    HOOKS_FEATURE, LAUNCH_FLAGS, SERVER_FLAGS, TUI_FLAGS,
};

/// How long each Codex command the probe runs may take.
const COMMAND_DEADLINE: Duration = Duration::from_secs(5);

/// What the Codex at `program`, running `version`, lacks of what the adapter relies on: each
/// launch flag in `codex --help` and `codex resume --help`, each server flag in
/// `codex app-server --help`, the hooks feature in `codex features list`, and the compaction
/// threshold key, which a Codex that knows it refuses by name when given a word instead of a
/// percent (#783). An error when Codex cannot be asked, which says nothing about the version.
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
        if !documents_flag(&help, CONFIG_FLAG) {
            missing.push(Missing {
                requirement: format!("flag {CONFIG_FLAG} in `codex {}`", command.join(" ")),
                blocks: vec![Capability::CompactionThreshold],
            });
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
    let probe_config = format!("{COMPACTION_THRESHOLD_KEY}={COMPACTION_THRESHOLD_PROBE_VALUE}");
    let refusal = command_output(
        program,
        &[CONFIG_FLAG, &probe_config, "features", "list"],
        COMMAND_DEADLINE,
    )?;
    if !documents_flag(&refusal, COMPACTION_THRESHOLD_KEY) {
        missing.push(Missing {
            requirement: format!(
                "key {COMPACTION_THRESHOLD_KEY} in `codex {CONFIG_FLAG}`, refused by name when \
                 not a percent"
            ),
            blocks: vec![Capability::CompactionThreshold],
        });
    }
    Ok(Probe::new(version, missing))
}
