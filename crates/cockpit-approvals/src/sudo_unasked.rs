//! "sudo ran without asking you" (#1374): the run's `sudo` launcher and the request's rewrite
//! give every sudo `-A -k`, but a script may call the trusted sudo by its path and so reuse an
//! unlock an earlier sudo in the run left. That cannot be prevented, so it is reported: while
//! a run runs, its process tree is watched for the trusted sudo, and a sudo that ended having
//! asked nothing through Torlek's helper is counted in its result. Whatever let it in, a
//! reused unlock or a password the command supplied itself (its own askpass, `-S`), the owner
//! was not asked.
//!
//! Not looked at: what a sudo runs, which runs as root and needs no unlock (`sudo` within
//! `sudo`), and a sudo's own children before they exec.
//!
//! A sudo is told by its process name and effective uid: its executable cannot be read, a
//! setuid process being closed to its user. So what it misses: the watch looks every
//! [`WATCH_POLL`], so a sudo shorter than that can pass unseen; a sudo outside the run's tree
//! (`setsid`, a daemonised child, one left to another parent) is not looked for; and one run
//! through a link of another name is not recognised. What it over-counts: another program
//! named like sudo that runs as root, and a sudo still running when the run ends, which may
//! ask yet.
//!
//! sudo changes its real and effective uids while it runs, so once seen, a sudo counts as
//! running until its pid no longer holds a process of its start time, whatever one look found.

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::time::Duration;

use crate::pty::LiveRun;
use crate::sudo::{Process, SudoAskpass, read_process, trusted_name};

/// How often a running run's process tree is read. sudo, loading its policy and PAM, takes
/// longer than this even when it reuses an unlock.
pub(crate) const WATCH_POLL: Duration = Duration::from_millis(10);
/// The most processes looked at in one look at a run's tree.
const TREE_MAX: usize = 4096;

/// One sudo process: its pid, and its start, which tells it from a later one with that pid.
type SudoKey = (u32, u64);

/// The trusted sudo's processes running in one run, those that asked the owner, and how many
/// ended without asking.
#[derive(Debug, Default)]
pub(crate) struct SudoSightings {
    running: BTreeSet<SudoKey>,
    /// Kept until the sudo that asked is seen gone.
    asked: BTreeSet<SudoKey>,
    ran_unasked: u64,
}

impl SudoSightings {
    /// `sudo` asked the owner through the helper.
    pub(crate) fn asked(&mut self, sudo: &Process) {
        self.asked.insert((sudo.pid, sudo.start));
    }

    /// The sudo processes that ended having asked the owner nothing. One still running may
    /// ask yet, so it is not counted.
    pub(crate) fn ran_unasked(&self) -> u64 {
        self.ran_unasked
    }

    /// The run ended: every sudo seen in it counts as ended, even one left running.
    pub(crate) fn end(&mut self) {
        for sudo in std::mem::take(&mut self.running) {
            self.ended(sudo);
        }
    }

    /// Records one look at the tree: the sudo processes running now.
    fn looked(&mut self, found: BTreeSet<SudoKey>) {
        let ended: Vec<SudoKey> = self.running.difference(&found).copied().collect();
        for sudo in ended {
            self.running.remove(&sudo);
            self.ended(sudo);
        }
        self.running.extend(found);
    }

    fn ended(&mut self, sudo: SudoKey) {
        if !self.asked.remove(&sudo) {
            self.ran_unasked = self.ran_unasked.saturating_add(1);
        }
    }
}

/// Whether the watch can see a run's tree: it reads each process's children from `/proc`,
/// which a kernel built without `CONFIG_PROC_CHILDREN` does not list, leaving every run
/// reported as asking.
pub fn unasked_sudo_watch_holds() -> Result<(), String> {
    let pid = std::process::id();
    std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
        .map(drop)
        .map_err(|error| {
            format!("a process's children cannot be read from /proc ({error}), so a sudo that ran without asking goes unreported")
        })
}

/// Records each trusted sudo in the tree under the run's session leader `leader`, until the
/// returned future is dropped with the run. Each look reads `/proc` on the blocking pool.
pub(crate) async fn watch(askpass: &SudoAskpass, live: &LiveRun, leader: u32) -> Infallible {
    let name = trusted_name(&askpass.sudo);
    let owner = askpass.sudo_owner;
    let mut looks = tokio::time::interval(WATCH_POLL);
    looks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        looks.tick().await;
        let name = name.clone();
        let running = live.sudo_sightings().running.clone();
        let look = move || {
            let mut found = sudos_under(leader, &name, owner);
            found.extend(running.into_iter().filter(|sudo| still_runs(*sudo)));
            found
        };
        // A look that panicked found nothing new; the next one looks again.
        if let Ok(found) = tokio::task::spawn_blocking(look).await {
            live.sudo_sightings().looked(found);
        }
    }
}

/// The sudo processes in the tree under `leader`: named `name` and running as `owner`. What a
/// process so named runs is not looked at, whatever uid it holds at the time.
fn sudos_under(leader: u32, name: &str, owner: u32) -> BTreeSet<SudoKey> {
    let mut found = BTreeSet::new();
    let mut looked_at = 0;
    let mut waiting = vec![leader];
    while let Some(pid) = waiting.pop() {
        if looked_at == TREE_MAX {
            break;
        }
        looked_at += 1;
        let Some(process_name) = name_of(pid) else {
            continue;
        };
        if process_name != name {
            waiting.extend(children_of(pid));
            continue;
        }
        if let Ok(sudo) = read_process(pid)
            && sudo.effective_uid == owner
        {
            found.insert((pid, sudo.start));
        }
    }
    found
}

/// The name `/proc/<pid>/comm` gives, read before anything else about a process.
fn name_of(pid: u32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.strip_suffix('\n').unwrap_or(&comm).to_owned())
}

/// The sudo `sudo` still runs: its pid holds a process of its start.
fn still_runs((pid, start): SudoKey) -> bool {
    read_process(pid).is_ok_and(|process| process.start == start)
}

fn children_of(pid: u32) -> Vec<u32> {
    let Ok(threads) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
        return Vec::new();
    };
    threads
        .flatten()
        .filter_map(|thread| std::fs::read_to_string(thread.path().join("children")).ok())
        .flat_map(|children| {
            children
                .split_whitespace()
                .filter_map(|child| child.parse().ok())
                .collect::<Vec<u32>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ended_sudo_counts_only_when_it_asked_nothing() {
        let mut sightings = SudoSightings::default();
        sightings.looked(BTreeSet::from([(10, 1), (12, 1)]));
        sightings.asked.insert((12, 1));
        // A later process with the first one's pid is another sudo.
        sightings.asked.insert((10, 2));

        sightings.looked(BTreeSet::new());

        assert_eq!(sightings.ran_unasked(), 1);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_seen_sudo_runs_while_its_pid_holds_a_process_of_its_start() {
        let this = read_process(std::process::id()).unwrap();

        assert!(still_runs((this.pid, this.start)));
        assert!(!still_runs((this.pid, this.start + 1)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn this_kernel_lists_a_process_s_children() {
        assert_eq!(unasked_sudo_watch_holds(), Ok(()));
    }

    #[test]
    fn a_sudo_still_running_is_not_counted_until_it_ends() {
        let mut sightings = SudoSightings::default();
        sightings.looked(BTreeSet::from([(10, 1)]));
        sightings.looked(BTreeSet::from([(10, 1)]));
        assert_eq!(sightings.ran_unasked(), 0);

        sightings.end();

        assert_eq!(sightings.ran_unasked(), 1);
    }
}
