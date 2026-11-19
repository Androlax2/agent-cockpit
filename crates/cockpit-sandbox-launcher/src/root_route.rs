//! A confined run's way to root (#1387): its `sudo` hands the root part to the daemon, which
//! asks the owner's password through sudo for each one.
//!
//! - `torlek-sandbox run-root-route FOLDER SUDO ASKPASS RUN TERM [--write DIR]... [--hide
//!   DIR]...`, started by the daemon before the run: listens on `FOLDER`'s socket, which only
//!   that run's service is shown, prints `ready`, then serves each `sudo` as it connects, so
//!   two in one pipeline run together:
//!   1. the run's `sudo` connects as the owner's user and sends
//!      [`ROOT_RUN_DESCRIPTORS_BYTE`] with its standard input, output and error, then one line
//!      of [hex words](encode_words): its folder and its arguments;
//!   2. it prints `ask N PID WORD...` (the same words); the daemon's decision comes on its
//!      standard input, in any order with the others: `go N HELPER WORD...` (the helper and
//!      the root command's words, hex) or `refuse N WHY` (hex);
//!   3. on `go` it runs `SUDO -A -k -p PROMPT -- HELPER root-run` with one end of a socket pair
//!      as its standard input and the run's standard error as its own, sends the helper the
//!      run's descriptors and the [`RootRunRequest`] (`writable` the `--write` folders, `hidden`
//!      the `--hide` folders), and passes on the helper's `refused:` or `exited CODE`;
//!   4. it prints `ended N WHAT`.
//!
//!   A run's `sudo` that goes while its root part runs closes the helper's socket, which stops
//!   the root part.
//! - `torlek-sandbox root-route-sudo FOLDER ARGUMENT...`, the run's `sudo`: the steps above
//!   from the run's side. It prints `sudo: WHY` and exits 1 when refused, and otherwise exits
//!   with the root part's code.
//!
//! The decision is the daemon's: this checks only that the run's `sudo` runs as this user.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, IoSlice, Read, Write};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cockpit_sandbox::{MAX_ROOT_RUN_REQUEST_BYTES, ROOT_RUN_DESCRIPTORS_BYTE, RootRunRequest};
use rustix::net::{SendAncillaryBuffer, SendAncillaryMessage, SendFlags, sendmsg};

use crate::handoff;

/// The socket in `FOLDER` the run's `sudo` connects to.
pub const SOCKET_NAME: &str = "root-route.sock";

/// What `run-root-route` prints once the run's `sudo` can connect.
pub const READY: &str = "ready";

/// The root part's search for a program named without a folder: the helper's own `PATH`,
/// never the run's.
const ROOT_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin";

/// How long the run's `sudo` has to send its words once connected.
const WORDS_DEADLINE: Duration = Duration::from_secs(10);

/// The standard input, output and error a root part gets, in that order.
const RUN_DESCRIPTORS: usize = 3;

/// The longest root command a prompt quotes.
const PROMPT_COMMAND_CHARS: usize = 80;

/// What the run's `sudo` exits with when it ran no root part.
const REFUSED_EXIT: u8 = 1;

/// Words as one line: each word's bytes in lower-case hex, the words parted by spaces. A word
/// may hold any byte, a space or a newline among them, and still travels on one line.
pub fn encode_words<'w>(words: impl IntoIterator<Item = &'w [u8]>) -> String {
    words
        .into_iter()
        .map(|word| word.iter().map(|byte| format!("{byte:02x}")).collect())
        .collect::<Vec<String>>()
        .join(" ")
}

/// The words [`encode_words`] made `line` of.
pub fn decode_words(line: &str) -> Result<Vec<Vec<u8>>, String> {
    line.split(' ')
        .filter(|word| !word.is_empty())
        .map(|word| {
            if word.len() % 2 != 0 || !word.is_ascii() {
                return Err(format!("{word:?} is no hex word"));
            }
            (0..word.len())
                .step_by(2)
                .map(|at| {
                    u8::from_str_radix(&word[at..at + 2], 16)
                        .map_err(|error| format!("{word:?} is no hex word: {error}"))
                })
                .collect()
        })
        .collect()
}

fn text(word: Vec<u8>, what: &str) -> Result<String, String> {
    String::from_utf8(word).map_err(|_| format!("the {what} is not UTF-8"))
}

/// What the relay serves each root part with.
struct Relay {
    sudo: PathBuf,
    askpass: PathBuf,
    run: u64,
    term: Option<String>,
    writable: Vec<PathBuf>,
    hidden: Vec<PathBuf>,
    uid: u32,
}

pub fn run(arguments: &[OsString]) -> Result<ExitCode, String> {
    let [folder, sudo, askpass, run, term, rest @ ..] = arguments else {
        return Err(
            "run-root-route needs FOLDER SUDO ASKPASS RUN TERM [--write DIR]... [--hide DIR]..."
                .to_owned(),
        );
    };
    let (writable, hidden) = folders(rest)?;
    let run = run
        .to_str()
        .and_then(|run| run.parse().ok())
        .ok_or_else(|| format!("the run {run:?} is no number"))?;
    let term = term
        .to_str()
        .ok_or_else(|| "TERM is not UTF-8".to_owned())?;
    let relay = Relay {
        sudo: PathBuf::from(sudo),
        askpass: PathBuf::from(askpass),
        run,
        term: (!term.is_empty()).then(|| term.to_owned()),
        writable,
        hidden,
        uid: crate::root_run::effective_uid()?,
    };
    let listener = handoff::in_folder(Path::new(folder), SOCKET_NAME, |socket| {
        match std::fs::remove_file(&socket) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        UnixListener::bind(socket)
    })
    .map_err(|error| format!("cannot listen in {}: {error}", Path::new(folder).display()))?;
    say(READY)?;
    let waiting = Waiting::default();
    accept(listener, Arc::new(relay), Arc::clone(&waiting))?;
    let mut decisions = std::io::stdin().lock();
    loop {
        let (ask, decided) = decision(&mut decisions).map_err(|Stop(reason)| reason)?;
        let Some(asking) = waiting
            .lock()
            .expect("the waiting lock is never poisoned")
            .remove(&ask)
        else {
            return Err(format!(
                "the daemon answered ask {ask}, which waits for nothing"
            ));
        };
        // Its thread waits for this alone, and only goes once it has it.
        let _ = asking.send(decided);
    }
}

/// Each `sudo` of the run waiting for the daemon's decision, by its ask's number.
type Waiting = Arc<Mutex<HashMap<u64, Sender<Decision>>>>;

/// Takes each `sudo` of the run as it connects, on a thread of its own. A relay that can take
/// no more stops: the run's next `sudo` would wait for nothing.
fn accept(listener: UnixListener, relay: Arc<Relay>, waiting: Waiting) -> Result<(), String> {
    std::thread::Builder::new()
        .name("root-route-accept".to_owned())
        .spawn(move || {
            for (ask, connection) in (1_u64..).zip(listener.incoming()) {
                let connection = match connection {
                    Ok(connection) => connection,
                    Err(error) => {
                        eprintln!("torlek-sandbox: cannot take a run's sudo: {error}");
                        std::process::exit(125);
                    }
                };
                let (relay, waiting) = (Arc::clone(&relay), Arc::clone(&waiting));
                let served = std::thread::Builder::new()
                    .name(format!("root-route-ask-{ask}"))
                    .spawn(move || {
                        let ended = relay.serve(&connection, ask, &waiting);
                        // A daemon that cannot be told is gone, which ends the relay.
                        let _ = say(&format!("ended {ask} {ended}"));
                    });
                if let Err(error) = served {
                    eprintln!("torlek-sandbox: cannot serve a run's sudo: {error}");
                    std::process::exit(125);
                }
            }
        })
        .map(drop)
        .map_err(|error| format!("cannot take the run's sudo: {error}"))
}

/// The `--write` and `--hide` folders, each absolute.
fn folders(arguments: &[OsString]) -> Result<(Vec<PathBuf>, Vec<PathBuf>), String> {
    let (mut writable, mut hidden) = (Vec::new(), Vec::new());
    let mut rest = arguments.iter();
    while let Some(flag) = rest.next() {
        let into = match flag.to_str() {
            Some("--write") => &mut writable,
            Some("--hide") => &mut hidden,
            _ => {
                return Err(format!(
                    "run-root-route takes --write or --hide, not {flag:?}"
                ));
            }
        };
        let folder = PathBuf::from(
            rest.next()
                .ok_or_else(|| format!("{flag:?} needs a folder"))?,
        );
        if !folder.is_absolute() {
            return Err(format!("{} is not absolute", folder.display()));
        }
        into.push(folder);
    }
    Ok((writable, hidden))
}

fn say(line: &str) -> Result<(), String> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{line}")
        .and_then(|()| stdout.flush())
        .map_err(|error| format!("cannot tell the daemon: {error}"))
}

/// Why the relay stops serving: the daemon is gone or spoke out of turn.
struct Stop(String);

/// What the run's `sudo` sent.
struct Asked {
    pid: u32,
    descriptors: Vec<OwnedFd>,
    words: Vec<Vec<u8>>,
}

impl Relay {
    /// One `sudo` of the run, to its end; what ended it, for the daemon's log.
    fn serve(&self, connection: &UnixStream, ask: u64, waiting: &Waiting) -> String {
        let asked = match self.asked(connection) {
            Ok(asked) => asked,
            Err(why) => return refuse(connection, &why),
        };
        let (decide, decided) = channel();
        waiting
            .lock()
            .expect("the waiting lock is never poisoned")
            .insert(ask, decide);
        let words = encode_words(asked.words.iter().map(Vec::as_slice));
        if let Err(why) = say(&format!("ask {ask} {} {words}", asked.pid)) {
            return refuse(connection, &why);
        }
        match decided.recv() {
            Err(_) => refuse(connection, "the daemon is gone"),
            Ok(Decision::Refuse(why)) => refuse(connection, &why),
            Ok(Decision::Go { helper, argv }) => {
                match self.root_part(connection, &asked, ask, &helper, argv) {
                    Ok(answer) => answer,
                    Err(why) => refuse(connection, &why),
                }
            }
        }
    }

    /// Who connected, checked to be this user, and what it sent.
    fn asked(&self, connection: &UnixStream) -> Result<Asked, String> {
        let peer = rustix::net::sockopt::socket_peercred(connection)
            .map_err(|error| format!("cannot tell who asked: {error}"))?;
        if peer.uid.as_raw() != self.uid {
            return Err(format!(
                "user {} is not this run's, {}",
                peer.uid.as_raw(),
                self.uid
            ));
        }
        let pid = u32::try_from(peer.pid.as_raw_nonzero().get())
            .map_err(|_| "the asking process has no pid".to_owned())?;
        connection
            .set_read_timeout(Some(WORDS_DEADLINE))
            .map_err(|error| format!("cannot wait for sudo's words: {error}"))?;
        let (byte, descriptors) = handoff::receive(connection, RUN_DESCRIPTORS)
            .map_err(|error| format!("cannot take the run's terminal: {error}"))?;
        if byte != ROOT_RUN_DESCRIPTORS_BYTE || descriptors.len() != RUN_DESCRIPTORS {
            return Err("the run's sudo sent no terminal".to_owned());
        }
        let mut line = String::new();
        BufReader::new(connection.take(MAX_ROOT_RUN_REQUEST_BYTES as u64))
            .read_line(&mut line)
            .map_err(|error| format!("cannot read sudo's words: {error}"))?;
        let words = decode_words(line.trim_end_matches('\n'))?;
        if words.is_empty() {
            return Err("the run's sudo sent no folder".to_owned());
        }
        connection
            .set_read_timeout(None)
            .map_err(|error| format!("cannot keep listening to sudo: {error}"))?;
        Ok(Asked {
            pid,
            descriptors,
            words,
        })
    }

    /// Runs the root part the daemon let go, through sudo: the helper's last answer, passed
    /// on; `Err` with why when no root part ran.
    fn root_part(
        &self,
        connection: &UnixStream,
        asked: &Asked,
        ask: u64,
        helper: &Path,
        mut argv: Vec<String>,
    ) -> Result<String, String> {
        let cwd = PathBuf::from(text(asked.words[0].clone(), "run's folder")?);
        argv[0] = root_program(&argv[0], &cwd)?;
        let request = RootRunRequest {
            run: self.run,
            ask,
            argv,
            cwd,
            writable: self.writable.clone(),
            hidden: self.hidden.clone(),
            term: self.term.clone(),
        };
        let (relay_side, helper_side) = UnixStream::pair()
            .map_err(|error| format!("cannot open the helper's socket: {error}"))?;
        let stderr = asked.descriptors[2]
            .try_clone()
            .map_err(|error| format!("cannot pass on the run's terminal: {error}"))?;
        let mut sudo = Command::new(&self.sudo)
            .args(["-A", "-k", "-p"])
            .arg(prompt(&request))
            .arg("--")
            .arg(helper)
            .arg(cockpit_sandbox::ROOT_RUN_VERB)
            .env_clear()
            .env("SUDO_ASKPASS", &self.askpass)
            .stdin(Stdio::from(OwnedFd::from(helper_side)))
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(|error| format!("cannot run {}: {error}", self.sudo.display()))?;
        let answer = send_request(&relay_side, &request, &asked.descriptors)
            .map_err(|error| format!("cannot send the helper the request: {error}"))
            .and_then(|()| relay_answers(connection, &relay_side));
        let status = sudo
            .wait()
            .map_err(|error| format!("cannot wait for sudo: {error}"))?;
        match answer {
            Ok(Some(answer)) => Ok(answer),
            Ok(None) => Err(format!("sudo ran no root part ({status})")),
            Err(why) => Err(format!("sudo ran no root part ({status}): {why}")),
        }
    }
}

/// Tells the run's `sudo` it was refused, and why; the same for the daemon's log.
fn refuse(connection: &UnixStream, why: &str) -> String {
    match writeln!(&*connection, "refused: {why}") {
        Ok(()) => format!("refused: {why}"),
        Err(error) => format!("refused: {why} (the run's sudo had gone: {error})"),
    }
}

/// The daemon's decision on one ask.
enum Decision {
    Go { helper: PathBuf, argv: Vec<String> },
    Refuse(String),
}

/// The daemon's next decision, and the number of the ask it answers.
fn decision(decisions: &mut impl BufRead) -> Result<(u64, Decision), Stop> {
    let mut line = String::new();
    match decisions.read_line(&mut line) {
        Ok(0) => return Err(Stop("the daemon is gone".to_owned())),
        Ok(_) => {}
        Err(error) => return Err(Stop(format!("cannot read the daemon: {error}"))),
    }
    let unreadable = || Stop(format!("the daemon answered {line:?}"));
    let mut fields = line.trim_end_matches('\n').splitn(3, ' ');
    let (Some(verb), Some(number), rest) = (fields.next(), fields.next(), fields.next()) else {
        return Err(unreadable());
    };
    let ask: u64 = number.parse().map_err(|_| unreadable())?;
    let words = decode_words(rest.unwrap_or_default()).map_err(Stop)?;
    let decided = match verb {
        "refuse" => {
            let why = words.into_iter().next().unwrap_or_default();
            Decision::Refuse(String::from_utf8_lossy(&why).into_owned())
        }
        "go" if words.len() >= 2 => {
            let mut words = words.into_iter();
            let helper =
                PathBuf::from(text(words.next().unwrap_or_default(), "helper").map_err(Stop)?);
            let argv = words
                .map(|word| text(word, "root command"))
                .collect::<Result<_, _>>()
                .map_err(Stop)?;
            Decision::Go { helper, argv }
        }
        _ => return Err(unreadable()),
    };
    Ok((ask, decided))
}

/// `program` as the helper runs it: a path from `cwd`, or found in [`ROOT_PATH`].
fn root_program(program: &str, cwd: &Path) -> Result<String, String> {
    if program.contains('/') {
        return Ok(cwd.join(program).display().to_string());
    }
    ROOT_PATH
        .split(':')
        .map(|folder| Path::new(folder).join(program))
        .find(|path| is_executable(path))
        .map(|path| path.display().to_string())
        .ok_or_else(|| format!("no {program} in the root part's PATH, {ROOT_PATH}"))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// sudo's prompt, naming the root part: the owner reads it in the queue. `%` is sudo's escape.
fn prompt(request: &RootRunRequest) -> String {
    let command = request.argv.join(" ");
    let mut quoted: String = command.chars().take(PROMPT_COMMAND_CHARS).collect();
    if quoted.len() < command.len() {
        quoted.push('…');
    }
    format!(
        "[sudo] root part of run {} ({}): password for %p: ",
        request.run,
        quoted.replace('%', "%%")
    )
}

fn send_request(
    helper: &UnixStream,
    request: &RootRunRequest,
    descriptors: &[OwnedFd],
) -> std::io::Result<()> {
    let borrowed: Vec<BorrowedFd<'_>> = descriptors.iter().map(AsFd::as_fd).collect();
    send_descriptors(helper, &borrowed)?;
    let mut line = serde_json::to_vec(request).map_err(std::io::Error::other)?;
    line.push(b'\n');
    (&*helper).write_all(&line)
}

/// [`ROOT_RUN_DESCRIPTORS_BYTE`] with the run's standard input, output and error.
fn send_descriptors(stream: &UnixStream, descriptors: &[BorrowedFd<'_>]) -> std::io::Result<()> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(RUN_DESCRIPTORS))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    if !control.push(SendAncillaryMessage::ScmRights(descriptors)) {
        return Err(std::io::Error::other("no room for the run's terminal"));
    }
    sendmsg(
        stream,
        &[IoSlice::new(&[ROOT_RUN_DESCRIPTORS_BYTE])],
        &mut control,
        SendFlags::empty(),
    )?;
    Ok(())
}

/// Passes the helper's `refused:` and `exited CODE` on to the run's `sudo`, and closes the
/// helper's socket when that `sudo` goes; the last line passed on, `None` when there was none.
fn relay_answers(run: &UnixStream, helper: &UnixStream) -> Result<Option<String>, String> {
    let watched = run
        .try_clone()
        .map_err(|error| format!("cannot watch the run's sudo: {error}"))?;
    let closing = helper
        .try_clone()
        .map_err(|error| format!("cannot watch the run's sudo: {error}"))?;
    std::thread::Builder::new()
        .name("run-sudo-gone".to_owned())
        .spawn(move || {
            let mut ignored = [0_u8; 64];
            while matches!((&watched).read(&mut ignored), Ok(read) if read > 0) {}
            let _ = closing.shutdown(std::net::Shutdown::Both);
        })
        .map_err(|error| format!("cannot watch the run's sudo: {error}"))?;
    let mut last = None;
    for line in BufReader::new(helper).lines() {
        let line = line.map_err(|error| format!("cannot read the helper's answers: {error}"))?;
        if line.starts_with("refused: ") || line.starts_with("exited ") {
            last = Some(match writeln!(&*run, "{line}") {
                Ok(()) => line,
                Err(error) => format!("{line} (the run's sudo had gone: {error})"),
            });
        }
    }
    Ok(last)
}

/// The run's `sudo`.
pub fn forward(arguments: &[OsString]) -> Result<ExitCode, String> {
    let [folder, rest @ ..] = arguments else {
        return Err("root-route-sudo needs FOLDER ARGUMENT...".to_owned());
    };
    let refused = |why: String| {
        eprintln!("sudo: {why}");
        ExitCode::from(REFUSED_EXIT)
    };
    let connection = match handoff::in_folder(Path::new(folder), SOCKET_NAME, UnixStream::connect) {
        Ok(connection) => connection,
        Err(error) => {
            return Ok(refused(format!(
                "cannot reach Torlek's root route ({error}): this run has none"
            )));
        }
    };
    let cwd = std::env::current_dir()
        .map_err(|error| format!("cannot tell the current folder: {error}"))?;
    let (stdin, stdout, stderr) = (std::io::stdin(), std::io::stdout(), std::io::stderr());
    send_descriptors(
        &connection,
        &[stdin.as_fd(), stdout.as_fd(), stderr.as_fd()],
    )
    .map_err(|error| format!("cannot hand Torlek the terminal: {error}"))?;
    use std::os::unix::ffi::OsStrExt;
    let words = encode_words(
        std::iter::once(cwd.as_os_str().as_bytes()).chain(rest.iter().map(|word| word.as_bytes())),
    );
    writeln!(&connection, "{words}")
        .map_err(|error| format!("cannot hand Torlek the command: {error}"))?;
    for line in BufReader::new(&connection).lines() {
        let line = line.map_err(|error| format!("cannot read Torlek's answer: {error}"))?;
        if let Some(why) = line.strip_prefix("refused: ") {
            return Ok(refused(why.to_owned()));
        }
        if let Some(code) = line.strip_prefix("exited ") {
            let code: i32 = code
                .parse()
                .map_err(|error| format!("the root part's exit {code:?}: {error}"))?;
            return Ok(ExitCode::from(u8::try_from(code).unwrap_or(REFUSED_EXIT)));
        }
    }
    Ok(refused(
        "Torlek's root route ended before the root part did".to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn any_word_travels_on_one_line() {
        let words: [&[u8]; 4] = [b"/home/o w", b"a\nb", b"", b"\xff%"];

        let line = encode_words(words);

        assert!(!line.contains('\n'));
        let decoded = decode_words(&line).unwrap();
        assert_eq!(decoded, [&b"/home/o w"[..], b"a\nb", b"\xff%"]);
    }

    #[test]
    fn a_word_that_is_no_hex_is_refused() {
        for line in ["abc", "zz", "0g", "aé0"] {
            assert!(decode_words(line).is_err(), "{line}");
        }
    }

    #[test]
    fn a_program_with_a_folder_is_taken_from_the_runs_folder() {
        assert_eq!(
            root_program("./install.sh", Path::new("/home/o/project")),
            Ok("/home/o/project/./install.sh".to_owned())
        );
    }

    #[test]
    fn a_bare_program_is_found_in_the_root_parts_path_alone() {
        assert!(
            root_program("sh", Path::new("/"))
                .unwrap()
                .starts_with("/usr/")
        );
        assert!(
            root_program("no-such-program-anywhere", Path::new("/"))
                .unwrap_err()
                .contains(ROOT_PATH)
        );
    }

    #[test]
    fn the_prompt_names_the_root_part_and_escapes_sudos_percent() {
        let request = RootRunRequest {
            run: 7,
            ask: 1,
            argv: vec!["/usr/bin/printf".to_owned(), "100%".to_owned()],
            cwd: PathBuf::from("/"),
            writable: Vec::new(),
            hidden: Vec::new(),
            term: None,
        };

        assert_eq!(
            prompt(&request),
            "[sudo] root part of run 7 (/usr/bin/printf 100%%): password for %p: "
        );
    }
}
