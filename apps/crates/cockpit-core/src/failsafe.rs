//! The failsafe: when a cockpit process (osd, the sidebar, greyproxy) stays over its CPU or memory
//! limit, every agent session is frozen and one fresh session is started to fix it. The sessions
//! thaw once the culprit is back under its limits for two minutes, on `cockpit-failsafe resume`, or
//! after thirty minutes at most. Checked once a minute by cockpit-failsafe.timer, outside the
//! daemon it watches: a runaway osd cannot watch itself.
//!
//! Why: on 2026-10-27 the sidebar retried a refused request 25 times a second and kept osd at
//! 30-45% CPU for an hour; nothing noticed until Theo did.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

pub struct Watched {
    pub role: &'static str,
    /// The process name as `/proc/<pid>/comm` shows it (15 characters at most).
    pub comm: &'static str,
    /// Share of one core, or None: greyproxy's CPU follows the agents' own traffic, so freezing
    /// them only hid it (its first trip on 2026-10-27 was that).
    pub cpu_limit_percent: Option<f64>,
    pub rss_limit_kb: u64,
}

const GIB_KB: u64 = 1024 * 1024;

/// osd sat at 30-45% of a core during the storm and near 6% after its fixes.
pub const WATCHED: &[Watched] = &[
    Watched {
        role: "osd",
        comm: "osd",
        cpu_limit_percent: Some(50.0),
        rss_limit_kb: 3 * GIB_KB / 2,
    },
    Watched {
        role: "sidebar",
        comm: "cockpit-sidebar",
        cpu_limit_percent: Some(50.0),
        rss_limit_kb: 3 * GIB_KB / 2,
    },
    Watched {
        role: "greyproxy",
        comm: "greyproxy",
        cpu_limit_percent: None,
        rss_limit_kb: 3 * GIB_KB,
    },
];

/// Over a limit for this long, in real time, before anything is frozen: counted in seconds, not
/// checks, since a check run by hand between two timer runs must not shorten it.
pub const SUSTAINED_S: i64 = 180;
/// A leak: memory grown by LEAK_STEP or more at each of this many checks in a row.
pub const LEAK_GROWTHS: usize = 10;
pub const LEAK_STEP: f64 = 1.10;
pub const CALM_FOR_S: i64 = 120;
pub const MAX_FREEZE_S: i64 = 30 * 60;
pub const COOLDOWN_S: i64 = 60 * 60;
const KEPT_SAMPLES: usize = LEAK_GROWTHS + 1;
/// USER_HZ: the unit of utime and stime in `/proc/<pid>/stat` on Linux.
const TICKS_PER_S: f64 = 100.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub at: i64,
    pub pid: String,
    pub cpu_ticks: u64,
    pub rss_kb: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trip {
    pub at: i64,
    pub culprit: String,
    pub reason: String,
    /// The systemd scopes this failsafe froze: only these are thawed again.
    pub frozen: Vec<String>,
    pub calm_since: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub samples: BTreeMap<String, Vec<Sample>>,
    pub tripped: Option<Trip>,
    pub last_tripped_at: Option<i64>,
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    Nothing,
    Trip { culprit: String, reason: String },
    Resume { why: String },
}

impl State {
    pub fn load(path: &Path) -> State {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::util::write_json(
            path,
            &serde_json::to_value(self).map_err(std::io::Error::other)?,
        )
    }

    /// Adds this check's sample of a role (None: not running, its history is dropped).
    pub fn record(&mut self, role: &str, sample: Option<Sample>) {
        let Some(sample) = sample else {
            self.samples.remove(role);
            return;
        };
        let history = self.samples.entry(role.to_string()).or_default();
        if history.last().is_some_and(|last| last.pid != sample.pid) {
            history.clear();
        }
        history.push(sample);
        let excess = history.len().saturating_sub(KEPT_SAMPLES);
        history.drain(..excess);
    }

    pub fn decide(&mut self, now: i64) -> Decision {
        if let Some(trip) = &mut self.tripped {
            let watched = WATCHED.iter().find(|watched| watched.role == trip.culprit);
            let history = self
                .samples
                .get(&trip.culprit)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let is_calm = watched.is_none_or(|watched| is_calm(history, watched));
            trip.calm_since = if is_calm {
                trip.calm_since.or(Some(now))
            } else {
                None
            };
            if trip
                .calm_since
                .is_some_and(|since| now - since >= CALM_FOR_S)
            {
                return Decision::Resume {
                    why: format!(
                        "{} back under its limits for {} minutes",
                        trip.culprit,
                        CALM_FOR_S / 60
                    ),
                };
            }
            if now - trip.at >= MAX_FREEZE_S {
                return Decision::Resume {
                    why: format!(
                        "{} minutes passed; {} may still be over its limits",
                        MAX_FREEZE_S / 60,
                        trip.culprit
                    ),
                };
            }
            return Decision::Nothing;
        }
        if self.last_tripped_at.is_some_and(|at| now - at < COOLDOWN_S) {
            return Decision::Nothing;
        }
        WATCHED
            .iter()
            .find_map(|watched| {
                let history = self.samples.get(watched.role)?;
                Some(Decision::Trip {
                    culprit: watched.role.to_string(),
                    reason: over_limit(history, watched)?,
                })
            })
            .unwrap_or(Decision::Nothing)
    }
}

/// Percent of one core between two samples of the same process.
fn cpu_percent(before: &Sample, after: &Sample) -> Option<f64> {
    let seconds = (after.at - before.at) as f64;
    (before.pid == after.pid && seconds > 0.0).then(|| {
        after.cpu_ticks.saturating_sub(before.cpu_ticks) as f64 / TICKS_PER_S / seconds * 100.0
    })
}

/// Why a process has been over its limits long enough to act, or None.
pub fn over_limit(history: &[Sample], watched: &Watched) -> Option<String> {
    let latest = history.last()?;
    if let Some(limit) = watched.cpu_limit_percent {
        let busy: Vec<&[Sample]> = history
            .windows(2)
            .rev()
            .take_while(|pair| {
                cpu_percent(&pair[0], &pair[1]).is_some_and(|percent| percent > limit)
            })
            .collect();
        if busy
            .last()
            .is_some_and(|earliest| latest.at - earliest[0].at >= SUSTAINED_S)
        {
            let lowest = busy
                .iter()
                .filter_map(|pair| cpu_percent(&pair[0], &pair[1]))
                .fold(f64::INFINITY, f64::min);
            return Some(format!(
                "CPU at {lowest:.0}% or more of a core for {} minutes (limit {limit:.0}%)",
                SUSTAINED_S / 60
            ));
        }
    }
    let heavy_since = history
        .iter()
        .rev()
        .take_while(|sample| sample.rss_kb > watched.rss_limit_kb)
        .last();
    if heavy_since.is_some_and(|earliest| latest.at - earliest.at >= SUSTAINED_S) {
        return Some(format!(
            "memory at {} MiB for {} minutes (limit {} MiB)",
            latest.rss_kb / 1024,
            SUSTAINED_S / 60,
            watched.rss_limit_kb / 1024
        ));
    }
    let growths = history
        .windows(2)
        .rev()
        .take_while(|pair| pair[1].rss_kb as f64 >= pair[0].rss_kb as f64 * LEAK_STEP)
        .count();
    if growths >= LEAK_GROWTHS {
        return Some(format!(
            "memory grew {:.0}% or more at each of {LEAK_GROWTHS} checks, now {} MiB (a leak)",
            (LEAK_STEP - 1.0) * 100.0,
            history.last()?.rss_kb / 1024
        ));
    }
    None
}

/// Under every limit at the latest check.
pub fn is_calm(history: &[Sample], watched: &Watched) -> bool {
    let Some(latest) = history.last() else {
        return true;
    };
    let is_cpu_calm = history.len() < 2
        || watched.cpu_limit_percent.is_none_or(|limit| {
            cpu_percent(&history[history.len() - 2], latest).is_none_or(|percent| percent <= limit)
        });
    is_cpu_calm && latest.rss_kb <= watched.rss_limit_kb
}

/// The process named `comm` under a /proc-like `root` with the most memory, sampled at `at`.
pub fn sample_process(root: &Path, comm: &str, at: i64) -> Option<Sample> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .filter(|name| name.bytes().all(|byte| byte.is_ascii_digit()))
                .map(str::to_string)
        })
        .filter(|pid| {
            std::fs::read_to_string(root.join(pid).join("comm"))
                .is_ok_and(|name| name.trim() == comm)
        })
        .filter_map(|pid| {
            let stat = std::fs::read_to_string(root.join(&pid).join("stat")).ok()?;
            let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
            let cpu_ticks =
                fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
            let status = std::fs::read_to_string(root.join(&pid).join("status")).ok()?;
            let rss_kb = status
                .lines()
                .find_map(|line| line.strip_prefix("VmRSS:"))?
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .ok()?;
            Some(Sample {
                at,
                pid,
                cpu_ticks,
                rss_kb,
            })
        })
        .max_by_key(|sample| sample.rss_kb)
}

/// What the fixer session is told: the culprit, the evidence and what it must not do.
pub fn fixer_brief(
    culprit: &str,
    reason: &str,
    history: &[Sample],
    load_lines: &[String],
    frozen: usize,
) -> String {
    let samples: Vec<String> = history
        .iter()
        .map(|sample| {
            format!(
                "  at {} pid {} cpu_ticks {} rss {} MiB",
                sample.at,
                sample.pid,
                sample.cpu_ticks,
                sample.rss_kb / 1024
            )
        })
        .collect();
    format!(
        "The cockpit failsafe froze {frozen} agent sessions: {culprit} is over its limits ({reason}).\n\
             Find why and fix it in agent-cockpit, with a regression or budget test, then apply it the usual way \
             (format-patch, then ask Theo to run cockpit apply with ask-to-run). The frozen sessions thaw on their own \
             once {culprit} stays under its limits for {} minutes, or after {} minutes at most; Theo can resume them \
             earlier with `cockpit-failsafe resume`. Every usual rule still holds: no install, git write or host command \
             without its approval.\n\nSamples (a minute apart):\n{}\n\nosd's latest load reports:\n{}",
        CALM_FOR_S / 60,
        MAX_FREEZE_S / 60,
        samples.join("\n"),
        load_lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const OSD: &Watched = &WATCHED[0];

    fn sample(minute: i64, cpu_ticks: u64, rss_mib: u64) -> Sample {
        Sample {
            at: minute * 60,
            pid: "42".into(),
            cpu_ticks,
            rss_kb: rss_mib * 1024,
        }
    }

    /// A history a minute apart where each minute used `percent` of a core.
    fn busy(percent: u64, minutes: i64) -> Vec<Sample> {
        (0..=minutes)
            .map(|minute| sample(minute, minute as u64 * percent * 60, 100))
            .collect()
    }

    fn state_with(role: &str, history: Vec<Sample>) -> State {
        State {
            samples: BTreeMap::from([(role.to_string(), history)]),
            ..State::default()
        }
    }

    #[test]
    fn three_minutes_over_the_cpu_limit_trip_it() {
        assert!(
            over_limit(&busy(60, 3), OSD).is_some_and(|reason| reason.starts_with("CPU at 60%"))
        );
    }

    #[test]
    fn two_minutes_over_the_cpu_limit_do_not() {
        assert_eq!(over_limit(&busy(60, 2), OSD), None);
    }

    #[test]
    fn normal_load_does_not_trip_it() {
        assert_eq!(over_limit(&busy(6, 10), OSD), None);
    }

    /// The first live trip (2026-10-27): two checks run by hand between timer runs made 70 s of
    /// samples count as three checks.
    #[test]
    fn checks_seconds_apart_do_not_shorten_the_three_minutes() {
        let history: Vec<Sample> = [0, 11, 61, 73]
            .iter()
            .map(|&at| Sample {
                at,
                pid: "42".into(),
                cpu_ticks: at as u64 * 95,
                rss_kb: 0,
            })
            .collect();
        assert_eq!(over_limit(&history, OSD), None);
    }

    #[test]
    fn greyproxys_cpu_follows_the_agents_traffic_and_never_trips_it() {
        assert_eq!(over_limit(&busy(95, 10), &WATCHED[2]), None);
    }

    #[test]
    fn memory_over_the_limit_for_three_minutes_trips_it() {
        let history: Vec<Sample> = (0..=3).map(|minute| sample(minute, 0, 1600)).collect();
        assert!(
            over_limit(&history, OSD)
                .is_some_and(|reason| reason.starts_with("memory at 1600 MiB"))
        );
    }

    #[test]
    fn memory_over_the_limit_for_two_minutes_does_not() {
        let history: Vec<Sample> = (0..=2).map(|minute| sample(minute, 0, 1600)).collect();
        assert_eq!(over_limit(&history, OSD), None);
    }

    #[test]
    fn memory_growing_ten_percent_at_ten_checks_is_a_leak() {
        let mut rss_kb = 100 * 1024;
        let history: Vec<Sample> = (0..=10)
            .map(|minute| {
                let at = Sample {
                    rss_kb,
                    ..sample(minute, 0, 0)
                };
                rss_kb = (rss_kb as f64 * LEAK_STEP).ceil() as u64;
                at
            })
            .collect();
        assert!(over_limit(&history, OSD).is_some_and(|reason| reason.ends_with("(a leak)")));
    }

    #[test]
    fn a_restarted_process_starts_a_new_history() {
        let mut state = State::default();
        state.record("osd", Some(sample(0, 0, 100)));
        state.record(
            "osd",
            Some(Sample {
                pid: "43".into(),
                ..sample(1, 0, 100)
            }),
        );
        assert_eq!(state.samples["osd"].len(), 1);
    }

    #[test]
    fn a_sustained_culprit_trips_the_failsafe() {
        let mut state = state_with("osd", busy(60, 3));
        assert!(matches!(state.decide(180), Decision::Trip { culprit, .. } if culprit == "osd"));
    }

    #[test]
    fn no_second_trip_within_the_cooldown() {
        let mut state = State {
            last_tripped_at: Some(0),
            ..state_with("osd", busy(60, 3))
        };
        assert_eq!(state.decide(COOLDOWN_S - 1), Decision::Nothing);
    }

    fn tripped(at: i64) -> Trip {
        Trip {
            at,
            culprit: "osd".into(),
            reason: "x".into(),
            frozen: vec!["a.scope".into()],
            calm_since: None,
        }
    }

    #[test]
    fn sessions_resume_after_two_calm_minutes() {
        let mut state = State {
            tripped: Some(tripped(0)),
            ..state_with("osd", busy(5, 3))
        };
        assert_eq!(state.decide(60), Decision::Nothing);
        assert!(matches!(
            state.decide(60 + CALM_FOR_S),
            Decision::Resume { .. }
        ));
    }

    #[test]
    fn a_busy_minute_restarts_the_calm_count() {
        let mut state = State {
            tripped: Some(tripped(0)),
            ..state_with("osd", busy(5, 3))
        };
        state.decide(60);
        state.samples.insert("osd".into(), busy(60, 3));
        state.decide(120);
        assert_eq!(state.tripped.as_ref().unwrap().calm_since, None);
    }

    #[test]
    fn sessions_resume_after_thirty_minutes_at_most() {
        let mut state = State {
            tripped: Some(tripped(0)),
            ..state_with("osd", busy(60, 3))
        };
        assert!(
            matches!(state.decide(MAX_FREEZE_S), Decision::Resume { why } if why.contains("may still be over"))
        );
    }

    #[test]
    fn a_process_is_sampled_from_its_stat_and_status() {
        let root = std::env::temp_dir().join(format!("failsafe-proc-{}", std::process::id()));
        let process = root.join("4242");
        std::fs::create_dir_all(&process).unwrap();
        std::fs::write(process.join("comm"), "osd\n").unwrap();
        std::fs::write(
            process.join("stat"),
            "4242 (osd) S 1 4242 4242 0 -1 4194560 100 0 0 0 1500 250 0 0 20 0 9 0 100 0 0",
        )
        .unwrap();
        std::fs::write(process.join("status"), "Name:\tosd\nVmRSS:\t  143724 kB\n").unwrap();
        let sampled = sample_process(&root, "osd", 7);
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(
            sampled,
            Some(Sample {
                at: 7,
                pid: "4242".into(),
                cpu_ticks: 1750,
                rss_kb: 143724
            })
        );
    }
}
