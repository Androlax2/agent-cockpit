//! `sudo` in an approved run asks the owner every time, through Torlek's askpass helper
//! (#1374), never through the run's terminal: sudo's `pwfeedback` prompt reads keys one at a
//! time, which the terminal guard refuses (#1372), and a terminal's mode is a guess any
//! program can fake.
//!
//! - Each run's `PATH` starts with a `sudo` that runs the trusted sudo with `-A -k`, and a
//!   leading or `&&`-chained `sudo` in the asked command gets the same flags before the owner
//!   reads it ([`sudo_asks_every_time`]). `-A` runs the helper `SUDO_ASKPASS` names; `-k`
//!   ignores a cached unlock and stores none, so no later sudo in the run reuses this one.
//! - The helper hands sudo's prompt to the daemon, which asks the owner only when the
//!   helper's parent is the trusted sudo inside a running approved command's session
//!   ([`Runner::ask_sudo`]). Anything else is refused and asks the owner nothing.
//! - The owner's answer goes back to the helper, which writes it to sudo on its standard
//!   output, a pipe: never the run's terminal, its output, a file or a log.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::oneshot;

use crate::pty::{LiveRun, Runner, prompt_line};
use crate::{Refusal, RunId};

const SUDO_LAUNCHER: &str = "sudo";
const ASKPASS_LAUNCHER: &str = "askpass";
/// The flags every sudo in a run gets: ask through the helper, never reuse an unlock.
const ASKING_FLAGS: &str = "-A -k";
/// Asks one run may hold open at once: each needs its own sudo process, so more is a run
/// starting sudo after sudo, not the owner's typing speed.
const OPEN_ASKS_PER_RUN: usize = 8;
/// The search `dash` and Debian's `bash` make when `PATH` is unset.
const UNSET_PATH_SEARCH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
/// What the owner reads when sudo's prompt held nothing printable.
const NO_PROMPT: &str = "(sudo sent no readable prompt)";
/// How long an ask waits for a launching run's session to be known.
const LAUNCH_SETTLE: Duration = Duration::from_secs(5);
const LAUNCH_POLL: Duration = Duration::from_millis(20);
/// The longest process name `/proc` reports (`TASK_COMM_LEN` less its terminator).
const PROCESS_NAME_MAX_BYTES: usize = 15;

/// How a run's `sudo` asks the owner (#1374).
#[derive(Debug, Clone)]
pub struct SudoAskpass {
    /// The sudo each run's `sudo` runs, and the only parent the helper is answered for.
    pub sudo: PathBuf,
    /// The uid the trusted sudo runs as, which its file's owner and setuid bit give it: root
    /// for the host's sudo.
    pub sudo_owner: u32,
    /// Where the run's `sudo` and `askpass` launchers are written; first in each run's `PATH`.
    pub launchers: PathBuf,
    /// The helper and its arguments; sudo appends its prompt.
    pub helper: Vec<OsString>,
}

impl SudoAskpass {
    /// Writes the launchers again before each run, so one run that changed them changes no
    /// later run.
    pub(crate) fn write_launchers(&self) -> std::io::Result<()> {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.launchers)?;
        let sudo_words = [self.sudo.as_os_str(), "-A".as_ref(), "-k".as_ref()];
        write_script(&self.launchers.join(SUDO_LAUNCHER), &sudo_words)?;
        let helper_words: Vec<&OsStr> = self.helper.iter().map(OsString::as_os_str).collect();
        write_script(&self.launchers.join(ASKPASS_LAUNCHER), &helper_words)
    }

    /// Why the trusted sudo cannot be answered for, if it cannot: a daemon whose sudo is
    /// missing or not setuid leaves runs' `sudo` as it is rather than break every one.
    pub fn trusted_sudo_holds(&self) -> Result<(), String> {
        trusted_sudo_holds(self)
    }

    /// `environment` with the launchers first in `PATH` and `SUDO_ASKPASS` naming the helper.
    /// Without a `PATH` of its own, a run searches the launchers, then what a shell searches
    /// when `PATH` is unset.
    pub(crate) fn apply(&self, environment: &mut Vec<(OsString, OsString)>) {
        let mut inherited: Option<OsString> = None;
        environment.retain(|(name, value)| {
            if name == "PATH" {
                inherited = Some(value.clone());
            }
            name != "PATH" && name != "SUDO_ASKPASS"
        });
        let mut path = self.launchers.clone().into_os_string();
        path.push(":");
        path.push(inherited.unwrap_or_else(|| UNSET_PATH_SEARCH.into()));
        environment.push(("PATH".into(), path));
        environment.push((
            "SUDO_ASKPASS".into(),
            self.launchers.join(ASKPASS_LAUNCHER).into_os_string(),
        ));
    }
}

/// A `#!/bin/sh` script that execs `words`, each single-quoted, then its own arguments. It is
/// written beside its place and renamed over it, so a run starting meanwhile never reads half.
fn write_script(path: &Path, words: &[&OsStr]) -> std::io::Result<()> {
    static WRITES: AtomicU64 = AtomicU64::new(0);
    let mut script = b"#!/bin/sh\nexec".to_vec();
    for word in words {
        script.push(b' ');
        script.extend(single_quoted(word));
    }
    script.extend_from_slice(b" \"$@\"\n");
    let mut written = path.as_os_str().to_owned();
    written.push(format!(
        ".{}.{}",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    let written = PathBuf::from(written);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&written)?;
    file.write_all(&script)?;
    drop(file);
    std::fs::rename(&written, path)
}

fn single_quoted(word: &OsStr) -> Vec<u8> {
    let mut quoted = vec![b'\''];
    for byte in word.as_bytes() {
        match byte {
            b'\'' => quoted.extend_from_slice(b"'\\''"),
            other => quoted.push(*other),
        }
    }
    quoted.push(b'\'');
    quoted
}

/// `command` with `-A -k` after each `sudo` word that starts it or follows `&&` outside
/// quotes, so the owner approves the flags the run uses (#1374). A `sudo` with them already
/// is left as it is. Other `sudo`s, in a pipe, after `;` or in a script, still get them from
/// the run's `sudo` launcher.
pub fn sudo_asks_every_time(command: &str) -> String {
    let mut flagged = String::with_capacity(command.len() + ASKING_FLAGS.len() + 1);
    let mut is_segment_start = true;
    let mut quote: Option<char> = None;
    let mut is_escaped = false;
    let mut characters = command.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if is_segment_start && quote.is_none() && !character.is_whitespace() {
            is_segment_start = false;
            let rest = &command[index..];
            if let Some(after) = sudo_word(rest) {
                flagged.push_str("sudo");
                if !has_asking_flags(after) {
                    flagged.push(' ');
                    flagged.push_str(ASKING_FLAGS);
                }
                for _ in 1.."sudo".len() {
                    characters.next();
                }
                continue;
            }
        }
        flagged.push(character);
        if is_escaped {
            is_escaped = false;
            continue;
        }
        match (quote, character) {
            (Some('\''), '\'') => quote = None,
            (Some('\''), _) => {}
            (_, '\\') => is_escaped = true,
            (Some('"'), '"') => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            (None, '&') if characters.peek().is_some_and(|(_, next)| *next == '&') => {
                let (_, second) = characters.next().expect("peeked");
                flagged.push(second);
                is_segment_start = true;
            }
            _ => {}
        }
    }
    flagged
}

/// What follows `sudo` when `text` starts with that word.
fn sudo_word(text: &str) -> Option<&str> {
    let after = text.strip_prefix("sudo")?;
    match after.chars().next() {
        None => Some(after),
        Some(next) if next.is_whitespace() => Some(after),
        Some(_) => None,
    }
}

fn has_asking_flags(after_sudo: &str) -> bool {
    let mut words = after_sudo.split_whitespace();
    words.next() == Some("-A") && words.next() == Some("-k")
}

/// One process as `/proc` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Process {
    pid: u32,
    name: String,
    parent: u32,
    session: i32,
    /// Clock ticks after boot: with the pid, tells this process from a later one.
    start: u64,
    effective_uid: u32,
}

#[cfg(target_os = "linux")]
fn read_process(pid: u32) -> Result<Process, String> {
    let read = |file: &str| {
        std::fs::read_to_string(format!("/proc/{pid}/{file}"))
            .map_err(|error| format!("process {pid} cannot be read ({error})"))
    };
    let stat = read("stat")?;
    let status = read("status")?;
    parse_process(pid, &stat, &status).ok_or_else(|| format!("process {pid} reads unexpectedly"))
}

#[cfg(not(target_os = "linux"))]
fn read_process(pid: u32) -> Result<Process, String> {
    Err(format!(
        "process {pid} cannot be checked: sudo's askpass is answered on Linux only"
    ))
}

/// A process from its `/proc/<pid>/stat` line and `status` file (proc(5)).
fn parse_process(pid: u32, stat: &str, status: &str) -> Option<Process> {
    // The name, field 2, is in parentheses and may hold either: fields resume after the last.
    let (head, fields) = stat.rsplit_once(')')?;
    let (_, name) = head.split_once('(')?;
    let fields: Vec<&str> = fields.split_whitespace().collect();
    // Field 3 is the state, 4 the parent, 6 the session, 22 the start time.
    let field = |number: usize| fields.get(number - 3).copied();
    let effective_uid = status
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    Some(Process {
        pid,
        name: name.to_owned(),
        parent: field(4)?.parse().ok()?,
        session: field(6)?.parse().ok()?,
        start: field(22)?.parse().ok()?,
        effective_uid,
    })
}

/// A helper and the sudo that started it, as checked when it asked.
#[derive(Debug, Clone)]
struct AskingSudo {
    helper: Process,
    sudo: Process,
}

/// The helper `helper_pid` and its parent, when that parent is the trusted sudo: a process
/// running as the trusted sudo's owner under its name, in the helper's session, while the
/// trusted file is that owner's and setuid. A setuid process's executable cannot be read by
/// the user it runs for, so its effective uid stands for it: an unprivileged program gets
/// root's only by running a setuid-root file.
fn sudo_asking(askpass: &SudoAskpass, helper_pid: u32) -> Result<AskingSudo, String> {
    trusted_sudo_holds(askpass)?;
    let helper = read_process(helper_pid)?;
    let sudo = read_process(helper.parent)?;
    let name = trusted_name(&askpass.sudo);
    if sudo.name != name || sudo.effective_uid != askpass.sudo_owner {
        return Err(format!(
            "Torlek's askpass answers {} only, and its parent (process {}, {}, uid {}) is not \
             it: nothing was asked",
            askpass.sudo.display(),
            sudo.pid,
            sudo.name,
            sudo.effective_uid
        ));
    }
    if sudo.session != helper.session {
        return Err(format!(
            "process {} is in another session than the sudo that started it: nothing was asked",
            helper.pid
        ));
    }
    helper_holds(askpass, &helper, &sudo)?;
    Ok(AskingSudo { helper, sudo })
}

/// The helper runs the program `askpass.helper` names, and its standard output is a pipe no
/// process but the trusted sudo holds. sudo runs whatever `SUDO_ASKPASS` names, which a
/// command may set itself: an askpass of its own, or Torlek's with its output sent anywhere
/// but sudo, is refused before the owner is asked.
///
/// What it cannot stop: a process running as the owner may trace the helper, or open its
/// pipe through `/proc` after this check. Either is code in a command the owner approved,
/// which runs with the owner's rights and could take a password typed at a terminal as well.
fn helper_holds(askpass: &SudoAskpass, helper: &Process, sudo: &Process) -> Result<(), String> {
    let program = askpass
        .helper
        .first()
        .ok_or_else(|| "no askpass helper is set: nothing was asked".to_owned())?;
    let expected = std::fs::metadata(program).map_err(|error| {
        format!(
            "{} cannot be read ({error}): nothing was asked",
            Path::new(program).display()
        )
    })?;
    let running = std::fs::metadata(format!("/proc/{}/exe", helper.pid)).map_err(|error| {
        format!(
            "process {}'s program cannot be read ({error}): nothing was asked",
            helper.pid
        )
    })?;
    if (running.dev(), running.ino()) != (expected.dev(), expected.ino()) {
        return Err(format!(
            "process {} is not Torlek's askpass helper: nothing was asked",
            helper.pid
        ));
    }
    let output = std::fs::read_link(format!("/proc/{}/fd/1", helper.pid)).map_err(|error| {
        format!(
            "process {}'s output cannot be read ({error}): nothing was asked",
            helper.pid
        )
    })?;
    if !output.as_os_str().as_bytes().starts_with(b"pipe:[") {
        return Err(format!(
            "process {}'s output is {}, not a pipe to sudo: nothing was asked",
            helper.pid,
            output.display()
        ));
    }
    if let Some(holder) = pipe_holder(&output, &[helper.pid, sudo.pid])? {
        return Err(format!(
            "process {holder} holds the pipe sudo reads the password from: nothing was asked"
        ));
    }
    Ok(())
}

/// A process other than `expected` with `pipe` open, among those whose descriptors this
/// user may read: a setuid sudo's are not, and it is expected.
fn pipe_holder(pipe: &Path, expected: &[u32]) -> Result<Option<u32>, String> {
    let processes = std::fs::read_dir("/proc")
        .map_err(|error| format!("/proc cannot be read ({error}): nothing was asked"))?;
    for process in processes.flatten() {
        let Some(pid) = process
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if expected.contains(&pid) {
            continue;
        }
        let Ok(descriptors) = std::fs::read_dir(process.path().join("fd")) else {
            continue;
        };
        let holds = descriptors.flatten().any(|descriptor| {
            std::fs::read_link(descriptor.path()).is_ok_and(|target| target == pipe)
        });
        if holds {
            return Ok(Some(pid));
        }
    }
    Ok(None)
}

/// The trusted sudo is a file its owner alone may change, setuid to that owner.
fn trusted_sudo_holds(askpass: &SudoAskpass) -> Result<(), String> {
    let path = &askpass.sudo;
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("{} cannot be read ({error})", path.display()))?;
    let mode = metadata.mode();
    if metadata.uid() != askpass.sudo_owner || mode & 0o4000 == 0 || mode & 0o022 != 0 {
        return Err(format!(
            "{} is not setuid to uid {} and writable by it alone: nothing was asked",
            path.display(),
            askpass.sudo_owner
        ));
    }
    Ok(())
}

/// The name `/proc` gives a process started from `sudo`: its file name, cut to 15 bytes.
fn trusted_name(sudo: &Path) -> String {
    let name = sudo.file_name().map(OsStr::as_bytes).unwrap_or_default();
    String::from_utf8_lossy(&name[..name.len().min(PROCESS_NAME_MAX_BYTES)]).into_owned()
}

/// The helper still runs under the sudo it asked for: a sudo that ended meanwhile, or a
/// helper left to another parent, gets no answer.
fn still_asking(askpass: &SudoAskpass, asking: &AskingSudo) -> Result<(), String> {
    let gone = || "the sudo that asked ended before the owner answered".to_owned();
    let helper = read_process(asking.helper.pid).map_err(|_| gone())?;
    let sudo = read_process(asking.sudo.pid).map_err(|_| gone())?;
    if helper.start != asking.helper.start
        || helper.parent != asking.sudo.pid
        || sudo.start != asking.sudo.start
    {
        return Err(gone());
    }
    helper_holds(askpass, &helper, &sudo)
}

/// What sudo asks the owner while a run waits for the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SudoAsk {
    /// Counts from 1 in each run; an answer names it.
    pub ask: u64,
    /// sudo's prompt, as a terminal would show it, redacted.
    pub prompt: String,
}

/// The owner's answer to one ask. Its `Debug` leaves the password out.
pub struct SudoAnswered {
    pub request: RunId,
    pub ask: u64,
    pub password: String,
}

impl fmt::Debug for SudoAnswered {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SudoAnswered")
            .field("request", &self.request)
            .field("ask", &self.ask)
            .finish_non_exhaustive()
    }
}

/// A run's asks still waiting for the owner.
#[derive(Debug, Default)]
pub(crate) struct SudoAsks {
    last: u64,
    waiting: BTreeMap<u64, WaitingAsk>,
    /// The run ended: no ask opens any more.
    is_closed: bool,
}

#[derive(Debug)]
struct WaitingAsk {
    prompt: String,
    asking: AskingSudo,
    /// The owner's password, or why the ask ended unanswered.
    answer: oneshot::Sender<Result<String, String>>,
}

impl SudoAsks {
    /// Ends every ask: each helper waiting is told the command ended.
    pub(crate) fn close(&mut self) {
        self.is_closed = true;
        self.waiting.clear();
    }

    pub(crate) fn is_waiting(&self) -> bool {
        !self.waiting.is_empty()
    }
}

/// Closes its ask when the helper's wait ends, answered or not.
struct OpenAsk<'live> {
    live: &'live LiveRun,
    ask: u64,
}

impl Drop for OpenAsk<'_> {
    fn drop(&mut self) {
        self.live.sudo_asks().waiting.remove(&self.ask);
    }
}

impl Runner {
    /// The helper `helper_pid` asks for the owner's password with sudo's `prompt` (#1374).
    /// It waits until the owner answers ([`Runner::answer_sudo`]), the run ends, or the
    /// returned future is dropped (the helper went). Refused, with why, unless the helper's
    /// parent is the trusted sudo in a running approved command's session.
    pub async fn ask_sudo(&self, helper_pid: u32, prompt: &str) -> Result<SudoAnswered, String> {
        let Some(askpass) = &self.settings.sudo else {
            return Err("this daemon does not answer sudo's askpass".to_owned());
        };
        let asking = sudo_asking(askpass, helper_pid)?;
        let (request, live) = self
            .live_in_session(asking.sudo.session)
            .await
            .ok_or_else(|| {
                "the sudo that asked is not inside a running approved command: nothing was asked"
                    .to_owned()
            })?;
        let (answer, answered) = oneshot::channel();
        let ask = {
            let mut asks = live.sudo_asks();
            if asks.is_closed {
                return Err("the command ended: nothing was asked".to_owned());
            }
            if asks.waiting.len() >= OPEN_ASKS_PER_RUN {
                return Err(format!(
                    "the command has {OPEN_ASKS_PER_RUN} sudo asks waiting already: nothing \
                     was asked"
                ));
            }
            asks.last += 1;
            let prompt = prompt_line(prompt.as_bytes()).unwrap_or_else(|| NO_PROMPT.to_owned());
            let ask = asks.last;
            asks.waiting.insert(
                ask,
                WaitingAsk {
                    prompt,
                    asking: asking.clone(),
                    answer,
                },
            );
            ask
        };
        let _open = OpenAsk { live: &live, ask };
        let password = answered
            .await
            .map_err(|_| "the command ended before the owner answered".to_owned())??;
        // Checked again: time passed since `answer_sudo` checked before handing it over.
        still_asking(askpass, &asking)?;
        Ok(SudoAnswered {
            request,
            ask,
            password,
        })
    }

    /// The owner's `password` for ask `ask` of the run `id`: handed to the helper waiting on
    /// it, never typed into the run's terminal.
    /// The sudo and helper are checked first, so the owner learns when the password could
    /// not be handed on.
    pub fn answer_sudo(&self, id: RunId, ask: u64, password: String) -> Result<(), Refusal> {
        let live = self.live_run(id).ok_or(Refusal::NotRunning)?;
        let no_ask = Refusal::NoSudoAsk { request: id.0, ask };
        let waiting = live
            .sudo_asks()
            .waiting
            .remove(&ask)
            .ok_or(no_ask.clone())?;
        let checked = match &self.settings.sudo {
            Some(askpass) => still_asking(askpass, &waiting.asking),
            None => Err("this daemon does not answer sudo's askpass".to_owned()),
        };
        if let Err(why) = checked {
            let _ = waiting.answer.send(Err(why.clone()));
            return Err(Refusal::SudoStoppedAsking {
                request: id.0,
                ask,
                why,
            });
        }
        waiting.answer.send(Ok(password)).map_err(|_| no_ask)
    }

    /// The oldest ask the run `id` waits on, if any.
    pub fn sudo_ask(&self, id: RunId) -> Option<SudoAsk> {
        let live = self.live_run(id)?;
        let asks = live.sudo_asks();
        asks.waiting.iter().next().map(|(ask, waiting)| SudoAsk {
            ask: *ask,
            prompt: waiting.prompt.clone(),
        })
    }

    /// The live run whose terminal session is `session`. A run's session is known once its
    /// launch returns, and its command may run sudo before that: while a run is still
    /// launching, the lookup waits for it, up to [`LAUNCH_SETTLE`].
    async fn live_in_session(&self, session: i32) -> Option<(RunId, Arc<LiveRun>)> {
        let deadline = tokio::time::Instant::now() + LAUNCH_SETTLE;
        loop {
            let runs = self.live_runs();
            let is_launching = runs.iter().any(|(_, live)| live.session().is_none());
            if let Some(found) = runs
                .into_iter()
                .find(|(_, live)| live.session() == Some(session))
            {
                return Some(found);
            }
            if !is_launching || tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(LAUNCH_POLL).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_and_and_chained_sudo_ask_every_time() {
        for (asked, flagged) in [
            ("sudo pacman -Syu", "sudo -A -k pacman -Syu"),
            ("  sudo true", "  sudo -A -k true"),
            ("sudo", "sudo -A -k"),
            (
                "make && sudo make install",
                "make && sudo -A -k make install",
            ),
            ("sudo a&&sudo b", "sudo -A -k a&&sudo -A -k b"),
            ("sudo -A -k true", "sudo -A -k true"),
            ("sudo -k -A true", "sudo -A -k -k -A true"),
        ] {
            assert_eq!(sudo_asks_every_time(asked), flagged, "{asked:?}");
        }
    }

    #[test]
    fn sudo_in_quotes_or_other_words_is_left() {
        for asked in [
            "echo 'a && sudo b'",
            "echo \"a && sudo b\"",
            "echo a \\&& sudo b",
            "sudoedit /etc/hosts",
            "./sudo true",
            "echo sudo",
            "true; sudo b",
            "true || sudo b",
        ] {
            assert_eq!(sudo_asks_every_time(asked), asked, "{asked:?}");
        }
    }

    #[test]
    fn a_process_reads_from_its_stat_line_and_status() {
        let stat = "4242 (sudo (x)) S 4200 4242 4100 34816 4242 4194560 1 0 0 0 0 0 0 0 20 0 1 \
                    0 987654 1000 100";
        let status = "Name:\tsudo\nUid:\t1000\t0\t0\t0\nGid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(
            parse_process(4242, stat, status),
            Some(Process {
                pid: 4242,
                name: "sudo (x)".to_owned(),
                parent: 4200,
                session: 4100,
                start: 987_654,
                effective_uid: 0,
            })
        );
    }

    #[test]
    fn a_process_name_is_cut_as_the_kernel_cuts_it() {
        assert_eq!(trusted_name(Path::new("/usr/bin/sudo")), "sudo");
        assert_eq!(
            trusted_name(Path::new("/opt/a-very-long-sudo-name")),
            "a-very-long-sud"
        );
    }

    #[test]
    fn launcher_words_are_quoted_for_sh() {
        assert_eq!(single_quoted(OsStr::new("/a b")), b"'/a b'");
        assert_eq!(single_quoted(OsStr::new("it's")), b"'it'\\''s'");
    }

    fn askpass() -> SudoAskpass {
        SudoAskpass {
            sudo: PathBuf::from("/usr/bin/sudo"),
            sudo_owner: 0,
            launchers: PathBuf::from("/run/torlek/sudo"),
            helper: vec!["/bin/torlek-daemon".into()],
        }
    }

    fn path_of(environment: &[(OsString, OsString)]) -> Vec<&OsString> {
        environment
            .iter()
            .filter(|(name, _)| name == "PATH")
            .map(|(_, value)| value)
            .collect()
    }

    #[test]
    fn the_launchers_come_first_in_the_runs_path() {
        let mut environment = vec![("PATH".into(), "/usr/bin:/bin".into())];

        askpass().apply(&mut environment);

        assert_eq!(path_of(&environment), ["/run/torlek/sudo:/usr/bin:/bin"]);
    }

    #[test]
    fn a_run_without_a_path_still_finds_the_shells_programs() {
        let mut environment = Vec::new();

        askpass().apply(&mut environment);

        assert_eq!(
            path_of(&environment),
            [&OsString::from(format!(
                "/run/torlek/sudo:{UNSET_PATH_SEARCH}"
            ))]
        );
    }

    #[test]
    fn sudo_askpass_names_the_launcher_whatever_the_run_inherited() {
        let mut environment = vec![("SUDO_ASKPASS".into(), "/tmp/fake".into())];

        askpass().apply(&mut environment);

        let askpass: Vec<_> = environment
            .iter()
            .filter(|(name, _)| name == "SUDO_ASKPASS")
            .map(|(_, value)| value)
            .collect();
        assert_eq!(askpass, ["/run/torlek/sudo/askpass"]);
    }
}
