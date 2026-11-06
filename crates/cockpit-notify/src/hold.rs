//! Holds (report 19): while one stands, the notifier keeps quiet, and when the last one ends
//! it tells once, quietly, what still needs the owner (#62, owner's choice A). `away` is set
//! by `torlek notify away on|off`; every other hold is an owner's detector program, whose
//! exit status 0 means it stands (a meeting, a game). The daemon knows none of them.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use crate::notifier::{Notice, Notifier};

/// The hold the away file sets.
pub const AWAY: &str = "away";

/// How long a detector may take before it counts as not holding.
const DETECTOR_DEADLINE: Duration = Duration::from_secs(3);

/// A hold the owner's program detects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detector {
    pub name: String,
    pub program: PathBuf,
}

impl Detector {
    /// Parses `NAME=PROGRAM`, as `torlek-notify --hold` takes it.
    pub fn parse(argument: &str) -> Result<Detector, String> {
        let Some((name, program)) = argument.split_once('=') else {
            return Err(format!("--hold {argument}: use NAME=PROGRAM"));
        };
        if name.is_empty() || name == AWAY {
            return Err(format!(
                "--hold {argument}: a detector needs a name other than {AWAY}"
            ));
        }
        Ok(Detector {
            name: name.to_owned(),
            program: PathBuf::from(program),
        })
    }
}

/// Where the holds come from, and what each detector last answered, so a detector that
/// fails is logged once rather than at every refresh.
#[derive(Debug)]
pub struct HoldSources {
    away_file: Option<PathBuf>,
    detectors: Vec<Detector>,
    failing: BTreeSet<String>,
}

impl HoldSources {
    pub fn new(away_file: Option<PathBuf>, detectors: Vec<Detector>) -> HoldSources {
        HoldSources {
            away_file,
            detectors,
            failing: BTreeSet::new(),
        }
    }

    /// The holds that stand now: `away` while its file exists, and each detector that exits
    /// 0 within its deadline. One that cannot run, or runs too long, does not hold.
    pub async fn standing(&mut self) -> BTreeSet<String> {
        let mut standing = BTreeSet::new();
        if self.away_file.as_ref().is_some_and(|file| file.exists()) {
            standing.insert(AWAY.to_owned());
        }
        for detector in &self.detectors {
            match detect(detector).await {
                Ok(holds) => {
                    if self.failing.remove(&detector.name) {
                        tracing::info!(hold = detector.name, "the hold's detector runs again");
                    }
                    if holds {
                        standing.insert(detector.name.clone());
                    }
                }
                Err(problem) => {
                    if self.failing.insert(detector.name.clone()) {
                        tracing::warn!(
                            hold = detector.name,
                            program = %detector.program.display(),
                            problem,
                            "the hold's detector fails; the hold does not stand"
                        );
                    }
                }
            }
        }
        standing
    }
}

async fn detect(detector: &Detector) -> Result<bool, String> {
    let mut child = tokio::process::Command::new(&detector.program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| format!("cannot start it: {error}"))?;
    match tokio::time::timeout(DETECTOR_DEADLINE, child.wait()).await {
        Ok(Ok(status)) => Ok(status.success()),
        Ok(Err(error)) => Err(format!("cannot wait for it: {error}")),
        Err(_) => Err(format!(
            "it ran longer than {} seconds",
            DETECTOR_DEADLINE.as_secs()
        )),
    }
}

/// What the holds keep back.
#[derive(Debug, Default)]
pub struct Hold {
    standing: BTreeSet<String>,
    held: Vec<Notice>,
}

impl Hold {
    /// The notices to tell now: all of them while no hold stands; none while one does, kept
    /// for the summary.
    pub fn pass(&mut self, notices: &[Notice]) -> Vec<Notice> {
        if self.standing.is_empty() {
            return notices.to_vec();
        }
        for notice in notices {
            let is_kept = self
                .held
                .iter()
                .any(|held| held.session == notice.session && held.event == notice.event);
            if !is_kept {
                self.held.push(notice.clone());
            }
        }
        Vec::new()
    }

    /// Takes the holds that stand now. When the last one ends, returns what was held and
    /// still needs the owner, for one quiet summary; empty otherwise.
    pub fn set_standing(&mut self, standing: BTreeSet<String>, notifier: &Notifier) -> Vec<Notice> {
        if standing == self.standing {
            return Vec::new();
        }
        tracing::info!(
            holds = ?standing,
            held = self.held.len(),
            "the holds changed"
        );
        let has_ended = !self.standing.is_empty() && standing.is_empty();
        self.standing = standing;
        if !has_ended {
            return Vec::new();
        }
        std::mem::take(&mut self.held)
            .into_iter()
            .filter(|notice| notifier.still_raises(notice))
            .collect()
    }

    pub fn standing(&self) -> &BTreeSet<String> {
        &self.standing
    }
}
