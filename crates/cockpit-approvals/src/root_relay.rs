//! A confined run's root route (#1387): `torlek-sandbox run-root-route`, started by the
//! daemon before a run whose command line writes out a `sudo COMMAND`, on the host. The run's
//! `PATH` starts with a `sudo` of its own, which hands its command to the relay; the relay asks
//! the daemon, and runs what the daemon lets go through `/usr/bin/sudo -A -k -- HELPER
//! root-run`, so the owner types their password for each root part. The daemon lets go only a
//! `sudo` the line writes out, started by the run's own shell (`root_route.rs`), whose files
//! are still as approved.
//!
//! Each run's folder holds the relay's socket and the run's `sudo`, and only that run's
//! service is shown it: a run that reached another's would ask with another card.

use std::ffi::OsString;
use std::fmt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout};

use crate::root_route::{FileId, WrittenSudos, read_link, root_argv, started_by_the_run_shell};
use crate::runs::Approved;
use crate::sudo::SudoAskpass;
use crate::sudo_editable::{RootFile, changed_file};

/// How long the relay has to say it is ready, and the daemon to decide one ask.
const RELAY_DEADLINE: Duration = Duration::from_secs(10);
/// How long the relay's last words may take once the run is over: they are written already.
const LAST_WORDS: Duration = Duration::from_millis(200);

const READY: &str = "ready";

/// Finds the root-run helper as it is now, or says why there is none: installed and held by
/// root, or not.
pub type LocateHelper = Arc<dyn Fn() -> Result<PathBuf, String> + Send + Sync>;

/// How confined runs reach root.
#[derive(Clone)]
pub struct RootRoute {
    /// `torlek-sandbox`.
    pub launcher: PathBuf,
    /// Where each run's folder is made: a folder confined runs are shown empty, so no run sees
    /// another's.
    pub folders: PathBuf,
    /// What a root part cannot see beside the helper's floor.
    pub hidden: Vec<PathBuf>,
    pub locate: LocateHelper,
}

impl fmt::Debug for RootRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RootRoute")
            .field("launcher", &self.launcher)
            .field("folders", &self.folders)
            .field("hidden", &self.hidden)
            .finish_non_exhaustive()
    }
}

impl PartialEq for RootRoute {
    fn eq(&self, other: &RootRoute) -> bool {
        self.launcher == other.launcher
            && self.folders == other.folders
            && self.hidden == other.hidden
            && Arc::ptr_eq(&self.locate, &other.locate)
    }
}

impl Eq for RootRoute {}

/// One run's relay, serving until it is dropped.
pub(crate) struct RunRootRoute {
    child: Child,
    asks: Lines<BufReader<ChildStdout>>,
    decisions: ChildStdin,
    folder: PathBuf,
    /// Why each of the run's `sudo`s that reached no root part did not.
    refused: Vec<String>,
}

/// What a run's relay needs to decide its asks.
pub(crate) struct Decider {
    /// The run's leader, its shell; `None` when the run's session is not known, and then no
    /// `sudo` of it can be checked.
    pub(crate) leader: Option<u32>,
    pub(crate) launch: Vec<OsString>,
    pub(crate) shell: FileId,
    pub(crate) written: WrittenSudos,
    pub(crate) bound: Vec<RootFile>,
    pub(crate) cwd: PathBuf,
    pub(crate) locate: LocateHelper,
}

impl RootRoute {
    /// Starts `run`'s relay, its `sudo` asking through `askpass`, once the relay is ready.
    pub(crate) async fn start(
        &self,
        run: &Approved,
        askpass: &SudoAskpass,
        term: Option<&std::ffi::OsStr>,
    ) -> Result<RunRootRoute, String> {
        let folder = self.folders.join(format!("run-{}", run.id.0));
        match std::fs::remove_dir_all(&folder) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot remove {}: {error}", folder.display())),
        }
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(folder.join("bin"))
            .map_err(|error| format!("cannot create {}: {error}", folder.display()))?;
        let created = write_sudo(&self.launcher, &folder).and_then(|()| {
            let mut command = tokio::process::Command::new(&self.launcher);
            command
                .arg("run-root-route")
                .arg(&folder)
                .arg(askpass.sudo())
                .arg(askpass.askpass_launcher())
                .arg(run.id.0.to_string())
                .arg(term.unwrap_or_default());
            for write in &run.reach.writes {
                command.arg("--write").arg(write);
            }
            for hidden in &self.hidden {
                command.arg("--hide").arg(hidden);
            }
            command
                .env_clear()
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .map_err(|error| format!("cannot start {}: {error}", self.launcher.display()))
        });
        let mut child = match created {
            Ok(child) => child,
            Err(why) => {
                // No `RunRootRoute` owns the folder yet to remove it when dropped.
                let _ = std::fs::remove_dir_all(&folder);
                return Err(why);
            }
        };
        let decisions = child.stdin.take().expect("its stdin is piped");
        let asks = BufReader::new(child.stdout.take().expect("its stdout is piped")).lines();
        let mut route = RunRootRoute {
            child,
            asks,
            decisions,
            folder,
            refused: Vec::new(),
        };
        match tokio::time::timeout(RELAY_DEADLINE, route.asks.next_line()).await {
            Ok(Ok(Some(line))) if line == READY => Ok(route),
            Ok(Ok(_)) => Err(format!(
                "the run's root route did not start: {}",
                route.failure().await
            )),
            Ok(Err(error)) => Err(format!("cannot read the run's root route: {error}")),
            Err(_) => Err(format!(
                "the run's root route was not ready within {}s",
                RELAY_DEADLINE.as_secs()
            )),
        }
    }
}

/// The run's `sudo`, in `folder/bin`: it runs `launcher root-route-sudo folder ARGUMENT...`.
fn write_sudo(launcher: &Path, folder: &Path) -> Result<(), String> {
    let path = folder.join("bin").join("sudo");
    crate::sudo::write_script(
        &path,
        &[
            launcher.as_os_str(),
            "root-route-sudo".as_ref(),
            folder.as_os_str(),
        ],
    )
    .map_err(|error| format!("cannot write {}: {error}", path.display()))
}

impl RunRootRoute {
    /// The folder the run's service is shown, read-only.
    pub(crate) fn folder(&self) -> &Path {
        &self.folder
    }

    /// The folder that leads the run's `PATH`, with its `sudo`.
    pub(crate) fn bin(&self) -> PathBuf {
        self.folder.join("bin")
    }

    /// The relay's pid: the parent of each sudo it starts.
    pub(crate) fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// Answers the relay's asks with `decider`'s decisions, until the relay goes.
    pub(crate) async fn serve(&mut self, decider: Arc<Decider>) {
        while let Ok(Some(line)) = self.asks.next_line().await {
            let Some(ask) = line.strip_prefix("ask ") else {
                self.note(&line);
                continue;
            };
            let answer = decide(&decider, ask).await;
            if self.decisions.write_all(answer.as_bytes()).await.is_err() {
                return;
            }
        }
    }

    /// Keeps why a `sudo` of the run reached no root part, from the relay's `ended N WHAT`.
    fn note(&mut self, line: &str) {
        let Some(ended) = line.strip_prefix("ended ") else {
            return;
        };
        let Some((_, what)) = ended.split_once(' ') else {
            return;
        };
        if let Some(why) = what.strip_prefix("refused: ") {
            self.refused.push(why.to_owned());
        }
    }

    /// Ends the relay, the run being over: why each of its `sudo`s that reached no root part
    /// did not.
    pub(crate) async fn end(mut self) -> Vec<String> {
        while let Ok(Ok(Some(line))) = tokio::time::timeout(LAST_WORDS, self.asks.next_line()).await
        {
            self.note(&line);
        }
        let _ = self.child.kill().await;
        std::mem::take(&mut self.refused)
    }

    async fn failure(&mut self) -> String {
        match tokio::time::timeout(RELAY_DEADLINE, self.child.wait()).await {
            Ok(Ok(status)) => format!("it exited with {status}"),
            Ok(Err(error)) => error.to_string(),
            Err(_) => "it answered something else".to_owned(),
        }
    }
}

impl Drop for RunRootRoute {
    fn drop(&mut self) {
        // The child is killed on drop; its folder goes with it.
        let _ = std::fs::remove_dir_all(&self.folder);
    }
}

/// The daemon's line for the relay's `ask N PID WORD...`: `go N HELPER WORD...` or `refuse N
/// WHY`, hex words.
async fn decide(decider: &Arc<Decider>, ask: &str) -> String {
    let mut fields = ask.splitn(3, ' ');
    let number = fields.next().unwrap_or_default().to_owned();
    let pid = fields.next().and_then(|pid| pid.parse::<u32>().ok());
    let words = fields.next().unwrap_or_default().to_owned();
    let deciding = {
        let decider = Arc::clone(decider);
        tokio::task::spawn_blocking(move || {
            let pid = pid.ok_or_else(|| "the root route named no process".to_owned())?;
            decider.decide(pid, &decode_words(&words)?)
        })
    };
    let decided = match tokio::time::timeout(RELAY_DEADLINE, deciding).await {
        Ok(Ok(decided)) => decided,
        Ok(Err(error)) => Err(format!("the decision failed: {error}")),
        Err(_) => Err(format!(
            "the files it runs could not be read within {}s",
            RELAY_DEADLINE.as_secs()
        )),
    };
    match decided {
        Ok((helper, argv)) => {
            let helper = helper.into_os_string();
            let words = std::iter::once(std::os::unix::ffi::OsStrExt::as_bytes(helper.as_os_str()))
                .chain(argv.iter().map(String::as_bytes));
            format!("go {number} {}\n", encode_words(words))
        }
        Err(why) => format!("refuse {number} {}\n", encode_words([why.as_bytes()])),
    }
}

impl Decider {
    /// The helper and the root command for `words` (the run's folder, then what its `sudo`
    /// was given) of the run's process `pid`, or why it reaches no root part.
    fn decide(&self, pid: u32, words: &[Vec<u8>]) -> Result<(PathBuf, Vec<String>), String> {
        let texts = words
            .iter()
            .map(|word| String::from_utf8(word.clone()))
            .collect::<Result<Vec<String>, _>>()
            .map_err(|_| "its words are not UTF-8".to_owned())?;
        let Some((folder, arguments)) = texts.split_first() else {
            return Err("the root route named no folder".to_owned());
        };
        let leader = self.leader.ok_or_else(|| {
            "the run's terminal session is not known: nothing was asked".to_owned()
        })?;
        let folder = program_id(Path::new(folder))
            .map_err(|why| format!("its folder, as the root part would run in it: {why}"))?;
        started_by_the_run_shell(pid, leader, &self.launch, self.shell, folder, read_link)?;
        let root = root_argv(arguments)?;
        if !self.written.commands.contains(&root) {
            return Err(format!(
                "`sudo {}` is not a sudo the approved command line writes out: only those reach \
                 root",
                root.join(" ")
            ));
        }
        if let Some(changed) = changed_file(&self.bound) {
            return Err(format!(
                "{} changed after you approved",
                crate::path_from(changed, &self.cwd)
            ));
        }
        let helper = (self.locate)()?;
        Ok((helper, root))
    }
}

/// The file at `path`, by its device and inode.
pub(crate) fn program_id(path: &Path) -> Result<FileId, String> {
    std::fs::metadata(path)
        .map(|metadata| (metadata.dev(), metadata.ino()))
        .map_err(|error| format!("{} cannot be read ({error})", path.display()))
}

/// Words as one line, as `torlek-sandbox run-root-route` reads them: each word's bytes in
/// lower-case hex, parted by spaces.
fn encode_words<'w>(words: impl IntoIterator<Item = &'w [u8]>) -> String {
    words
        .into_iter()
        .map(|word| word.iter().map(|byte| format!("{byte:02x}")).collect())
        .collect::<Vec<String>>()
        .join(" ")
}

fn decode_words(line: &str) -> Result<Vec<Vec<u8>>, String> {
    line.split(' ')
        .filter(|word| !word.is_empty())
        .map(|word| {
            if word.len() % 2 != 0 || !word.is_ascii() {
                return Err("the root route sent a word that is no hex".to_owned());
            }
            (0..word.len())
                .step_by(2)
                .map(|at| {
                    u8::from_str_radix(&word[at..at + 2], 16)
                        .map_err(|_| "the root route sent a word that is no hex".to_owned())
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_go_to_hex_and_back() {
        let words: [&[u8]; 3] = [b"/home/o w", b"a\nb", b"\xff"];

        assert_eq!(decode_words(&encode_words(words)).unwrap(), words);
    }

    #[test]
    fn a_word_that_is_no_hex_is_refused() {
        for line in ["abc", "zz", "é0"] {
            assert!(decode_words(line).is_err(), "{line}");
        }
    }
}
