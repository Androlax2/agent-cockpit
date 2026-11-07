//! The runtime probe of a harness version that is not on the adapter's tested list (#732).
//!
//! The probe is cheap and needs no account: it finds the flags the adapter passes in the
//! harness's `--help`, the environment names and hook events the adapter relies on in its
//! program. A requirement not found is named, and it takes away the capabilities that need it;
//! one extra flag or event the adapter does not know changes nothing.

use std::fmt;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::{Capabilities, Capability, Provenance};

/// The most output of one command the probe reads; `--help` is a few kilobytes.
const MAX_OUTPUT_BYTES: u64 = 1 << 20;

/// How much of a program's file the probe reads at once.
const SCAN_CHUNK_BYTES: usize = 1 << 20;

/// One thing the adapter relies on that a probed version lacks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    /// What was not found and where, such as ``flag --effort in `claude --help` ``.
    pub requirement: String,
    /// The capabilities that cannot be offered without it.
    pub blocks: Vec<Capability>,
}

/// What probing one harness version found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    version: String,
    missing: Vec<Missing>,
}

impl Probe {
    pub fn new(version: &str, missing: Vec<Missing>) -> Probe {
        Probe {
            version: version.to_owned(),
            missing,
        }
    }

    /// The version that was probed.
    pub fn version(&self) -> &str {
        &self.version
    }

    pub fn missing(&self) -> &[Missing] {
        &self.missing
    }

    /// Whether every requirement was found.
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty()
    }

    /// Whether a requirement of `capability` was not found.
    pub fn blocks(&self, capability: Capability) -> bool {
        self.missing
            .iter()
            .any(|missing| missing.blocks.contains(&capability))
    }

    /// Why `capability` cannot be offered, for the owner: each requirement of it that was not
    /// found, such as ``flag --autocompact in `claude --help` is missing in 1.0.0``; `None`
    /// when they were all found.
    pub fn reason_against(&self, capability: Capability) -> Option<String> {
        let requirements: Vec<&str> = self
            .missing
            .iter()
            .filter(|missing| missing.blocks.contains(&capability))
            .map(|missing| missing.requirement.as_str())
            .collect();
        (!requirements.is_empty()).then(|| {
            format!(
                "{} is missing in {}",
                requirements.join(" and "),
                self.version
            )
        })
    }

    /// The probe's result in a sentence for the owner: every requirement found, or each one
    /// missing and what it takes away.
    pub fn summary(&self) -> String {
        if self.is_clean() {
            return format!(
                "{}: every required flag, name and event found",
                self.version
            );
        }
        let missing: Vec<String> = self
            .missing
            .iter()
            .map(|missing| {
                let taken_away = if missing.blocks.len() == Capability::ALL.len() {
                    "every capability".to_owned()
                } else {
                    let names: Vec<&str> = missing.blocks.iter().map(|c| c.as_str()).collect();
                    names.join(", ")
                };
                format!(
                    "{} is missing, taking away {taken_away}",
                    missing.requirement
                )
            })
            .collect();
        format!("{}: {}", self.version, missing.join("; "))
    }
}

impl Provenance {
    /// What an adapter reports overall for the version it runs: `Tested` on a version of
    /// `tested_versions`, `Probed` when `probe` of this version found every requirement,
    /// `Discovered` otherwise.
    pub fn for_adapter(
        version: Option<&str>,
        tested_versions: &str,
        probe: Option<&Probe>,
    ) -> Provenance {
        match (
            Provenance::for_version(version, tested_versions),
            version,
            probe,
        ) {
            (tested @ Provenance::Tested { .. }, _, _) => tested,
            (_, Some(version), Some(probe)) if probe.version() == version && probe.is_clean() => {
                Provenance::Probed {
                    version: version.to_owned(),
                }
            }
            _ => Provenance::Discovered,
        }
    }

    /// What an adapter running `version` reports for `capability`: `Tested` on a version of
    /// `tested_versions`; otherwise `None` when `probe` of this version lacks a requirement of
    /// the capability, `Probed` when it found them all, `Discovered` without a probe.
    pub fn for_probe(
        version: Option<&str>,
        tested_versions: &str,
        probe: Option<&Probe>,
        capability: Capability,
    ) -> Option<Provenance> {
        let tested = Provenance::for_version(version, tested_versions);
        if matches!(tested, Provenance::Tested { .. }) {
            return Some(tested);
        }
        match (version, probe) {
            (Some(version), Some(probe)) if probe.version() == version => {
                if probe.blocks(capability) {
                    None
                } else {
                    Some(Provenance::Probed {
                        version: version.to_owned(),
                    })
                }
            }
            _ => Some(Provenance::Discovered),
        }
    }
}

impl Capabilities {
    /// What an adapter running `version` reports of `offered`, each with the provenance of
    /// [`Provenance::for_probe`]; one the probe blocks is withheld with the probe's reason, so
    /// a refusal names the requirement the version lacks.
    pub fn from_probe(
        version: Option<&str>,
        tested_versions: &str,
        probe: Option<&Probe>,
        offered: impl IntoIterator<Item = Capability>,
    ) -> Capabilities {
        offered
            .into_iter()
            .fold(
                Capabilities::none(),
                |capabilities, capability| match Provenance::for_probe(
                    version,
                    tested_versions,
                    probe,
                    capability,
                ) {
                    Some(provenance) => capabilities.with(capability, provenance),
                    None => {
                        let reason = probe
                            .and_then(|probe| probe.reason_against(capability))
                            .expect("`for_probe` withholds only what the probe blocks");
                        capabilities.withholding(capability, reason)
                    }
                },
            )
    }
}

/// Whether `help` documents `flag` as a word of its own, not as the start of a longer flag.
pub fn documents_flag(help: &str, flag: &str) -> bool {
    let is_flag_byte = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_';
    help.match_indices(flag).any(|(start, _)| {
        let before = help.as_bytes()[..start].last().copied();
        let after = help.as_bytes().get(start + flag.len()).copied();
        !before.is_some_and(is_flag_byte) && !after.is_some_and(is_flag_byte)
    })
}

/// Why a probe could not run, as opposed to what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeError {
    pub reason: String,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl std::error::Error for ProbeError {}

/// What `program` `arguments` print on standard output and standard error, at most 1 MiB of
/// each. An exit status is not checked: some harnesses print `--help` to standard error.
pub fn command_output(
    program: &Path,
    arguments: &[&str],
    deadline: Duration,
) -> Result<String, ProbeError> {
    let command = format!("`{} {}`", program.display(), arguments.join(" "));
    let error = |reason: String| ProbeError {
        reason: format!("{command}: {reason}"),
    };
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|cause| error(format!("cannot run it: {cause}")))?;
    let stdout = read_in_background(child.stdout.take());
    let stderr = read_in_background(child.stderr.take());
    let ends_at = Instant::now() + deadline;
    loop {
        if child
            .try_wait()
            .map_err(|cause| error(format!("cannot wait for it: {cause}")))?
            .is_some()
        {
            break;
        }
        if Instant::now() > ends_at {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error(format!(
                "did not answer within {} ms",
                deadline.as_millis()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let remaining = || ends_at.saturating_duration_since(Instant::now());
    let gave_up = || error("its output did not end in time".to_owned());
    let stdout = stdout.recv_timeout(remaining()).map_err(|_| gave_up())?;
    let stderr = stderr.recv_timeout(remaining()).map_err(|_| gave_up())?;
    Ok(format!("{stdout}\n{stderr}"))
}

fn read_in_background(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = (&mut pipe).take(MAX_OUTPUT_BYTES).read_to_end(&mut bytes);
            // Keep draining, so a chatty child never blocks on a full pipe.
            let _ = std::io::copy(&mut pipe, &mut std::io::sink());
        }
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receiver
}

/// The `names` the file at `path` does not contain as a word of their own (not inside a longer
/// name such as `Stop` inside `StopFailure`), in `names` order. The file is read in chunks,
/// so a harness of hundreds of megabytes costs time, not memory; the read ends as soon as every
/// name is found. A name left in the file as dead text still counts as found.
pub fn names_missing_from_file<'a>(
    path: &Path,
    names: &[&'a str],
) -> Result<Vec<&'a str>, ProbeError> {
    let error = |cause: std::io::Error| ProbeError {
        reason: format!("cannot read {}: {cause}", path.display()),
    };
    let mut file = std::fs::File::open(path).map_err(error)?;
    let mut missing: Vec<&str> = names.to_vec();
    // One byte more than the longest name stays, to see what comes before a name that
    // starts a chunk's tail.
    let overlap = names.iter().map(|name| name.len()).max().unwrap_or(0);
    let mut window: Vec<u8> = Vec::with_capacity(SCAN_CHUNK_BYTES + overlap);
    let mut chunk = vec![0_u8; SCAN_CHUNK_BYTES];
    while !missing.is_empty() {
        let read = file.read(&mut chunk).map_err(error)?;
        let is_end = read == 0;
        window.extend_from_slice(&chunk[..read]);
        missing.retain(|name| !contains_word(&window, name.as_bytes(), is_end));
        if is_end {
            break;
        }
        let keep_from = window.len().saturating_sub(overlap);
        window.drain(..keep_from);
    }
    Ok(missing)
}

/// Whether `haystack` holds `needle` with no name character on either side. The window's
/// end is a side only at the file's end: before it, the next chunk may continue the word.
fn contains_word(haystack: &[u8], needle: &[u8], is_file_end: bool) -> bool {
    let is_name_byte = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let Some((&first, rest)) = needle.split_first() else {
        return true;
    };
    let Some(last_start) = haystack.len().checked_sub(needle.len()) else {
        return false;
    };
    (0..=last_start).any(|start| {
        let end = start + needle.len();
        haystack[start] == first
            && &haystack[start + 1..end] == rest
            && !start
                .checked_sub(1)
                .is_some_and(|before| is_name_byte(haystack[before]))
            && match haystack.get(end) {
                Some(&after) => !is_name_byte(after),
                None => is_file_end,
            }
    })
}
