//! Control-mode observation: one `tmux -C` client per observed session, read on its own
//! thread (report 03: the tmux reader is a long-lived blocking worker).
//!
//! The reader always drains tmux at once. A control client that stops reading makes tmux
//! stop reading the pane, so a slow consumer would slow the agent itself; tmux's own
//! `pause-after` does not reliably fire then, because the pane produces nothing more to age.
//! So when the consumer falls behind, the reader drops that pane's output and reports
//! [`ControlEvent::OutputLost`]; memory stays bounded by [`ObserveOptions::capacity`]. The
//! consumer re-captures the pane's screen, which [`crate::CaptureStream`] does.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::Error;
use crate::names::{PaneId, SessionName};
use crate::server::Server;

/// One thing the control-mode client reported. Events of a pane arrive in the order tmux
/// sent them; [`ControlEvent::OutputLost`] may arrive before events queued ahead of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ControlEvent {
    /// Bytes a pane's program wrote: untrusted display data, never a source of task state.
    Output { pane: PaneId, bytes: Vec<u8> },
    /// Some of the pane's output never reached the consumer: it fell behind, or tmux
    /// paused the pane for this client (`is_paused`; [`Observer::resume`] lets it continue).
    /// The screen is only right again after a fresh capture.
    OutputLost { pane: PaneId, is_paused: bool },
    /// The answer to a command this client sent, in sending order. `lines` is the
    /// command's output, or tmux's error message when `is_error`.
    Reply { is_error: bool, lines: Vec<Vec<u8>> },
    /// The client ended: the session closed, the server stopped, or it was killed.
    Exit { reason: Option<String> },
    /// Any other notification (`%layout-change`, `%continue`, ...), `%` removed. Dropped,
    /// like output, when the consumer is behind.
    Notification { name: String, arguments: String },
}

/// Flow control and buffering of one [`Observer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObserveOptions {
    /// tmux's own backstop: how far behind, in seconds, this client may fall before tmux
    /// pauses a pane (1 or more).
    pub pause_after_seconds: u32,
    /// Events held for the consumer; output past this is dropped and reported lost.
    pub capacity: usize,
}

impl Default for ObserveOptions {
    fn default() -> Self {
        ObserveOptions {
            pause_after_seconds: 2,
            capacity: 256,
        }
    }
}

/// How long [`Server::observe`] waits for tmux to attach its control client.
const ATTACH_TIMEOUT: Duration = Duration::from_secs(10);

/// What the reader thread hands the consumer. The reader never waits on the consumer: it
/// only appends here, so it always drains tmux.
#[derive(Default)]
struct Inbox {
    state: Mutex<InboxState>,
    changed: Condvar,
}

#[derive(Default)]
struct InboxState {
    /// Events in tmux's order. Replies and the exit are always kept: replies answer the
    /// consumer's own commands, so they are bounded by what it sent.
    events: VecDeque<ControlEvent>,
    /// Outputs and notifications in `events`; past `capacity` new ones are dropped.
    droppable: usize,
    capacity: usize,
    /// Panes whose output was lost and not yet reported, in the order it happened.
    losses: Vec<(PaneId, bool)>,
    /// tmux's answer to the `attach-session` the client was started with.
    attach: Option<Result<(), String>>,
    /// The reader ended: nothing more will be appended.
    is_closed: bool,
}

impl Inbox {
    fn lock(&self) -> MutexGuard<'_, InboxState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn update(&self, change: impl FnOnce(&mut InboxState)) {
        change(&mut self.lock());
        self.changed.notify_all();
    }

    /// Waits until `is_ready` holds or `deadline` passes (`None`: no deadline).
    fn wait_until(
        &self,
        deadline: Option<Instant>,
        mut is_ready: impl FnMut(&mut InboxState) -> bool,
    ) -> MutexGuard<'_, InboxState> {
        let mut state = self.lock();
        while !is_ready(&mut state) {
            state = match deadline {
                None => self
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
                Some(deadline) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        break;
                    }
                    self.changed
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
        state
    }
}

impl InboxState {
    fn push(&mut self, event: ControlEvent) {
        match event {
            ControlEvent::OutputLost { pane, is_paused } => self.record_loss(pane, is_paused),
            ControlEvent::Output { pane, .. } if self.droppable >= self.capacity => {
                self.record_loss(pane, false);
            }
            ControlEvent::Notification { .. } if self.droppable >= self.capacity => {}
            ControlEvent::Output { .. } | ControlEvent::Notification { .. } => {
                self.droppable += 1;
                self.events.push_back(event);
            }
            ControlEvent::Reply { .. } | ControlEvent::Exit { .. } => self.events.push_back(event),
        }
    }

    fn record_loss(&mut self, pane: PaneId, is_paused: bool) {
        match self.losses.iter_mut().find(|(lost, _)| *lost == pane) {
            Some((_, was_paused)) => *was_paused |= is_paused,
            None => self.losses.push((pane, is_paused)),
        }
    }

    /// A loss first: the consumer re-captures, and a capture is ordered after every event
    /// still queued, so reporting it early is safe.
    fn pop(&mut self) -> Option<ControlEvent> {
        if !self.losses.is_empty() {
            let (pane, is_paused) = self.losses.remove(0);
            return Some(ControlEvent::OutputLost { pane, is_paused });
        }
        let event = self.events.pop_front()?;
        if matches!(
            event,
            ControlEvent::Output { .. } | ControlEvent::Notification { .. }
        ) {
            self.droppable -= 1;
        }
        Some(event)
    }
}

/// `now + timeout`, or `None` when that is too far away to represent: wait without limit.
fn deadline_after(timeout: Duration) -> Option<Instant> {
    Instant::now().checked_add(timeout)
}

/// A read-only control-mode client attached to one session. It never resizes the session
/// (`ignore-size`) and cannot type into it (`read-only`). Dropping it ends the client and
/// nothing else.
pub struct Observer {
    child: Child,
    stdin: ChildStdin,
    inbox: Arc<Inbox>,
    reader: Option<JoinHandle<()>>,
}

impl Server {
    /// Attaches a control-mode client to `session` and returns once tmux has attached it.
    /// Only output written after that arrives; take a [`Server::capture`] or use
    /// [`Server::capture_stream`] for the screen.
    pub fn observe(
        &self,
        session: &SessionName,
        options: ObserveOptions,
    ) -> Result<Observer, Error> {
        if options.pause_after_seconds == 0 || options.capacity == 0 {
            return Err(Error::InvalidArgument {
                what: "observe options (pause_after_seconds and capacity must be at least 1)",
                value: format!("{options:?}"),
            });
        }
        let client_flags = format!(
            "read-only,ignore-size,pause-after={}",
            options.pause_after_seconds
        );
        let mut child = self
            .command()
            .args(["-C", "attach-session", "-f", &client_flags, "-t"])
            .arg(session.target())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| Error::TmuxNotRunnable {
                binary: self.tmux_binary().to_path_buf(),
                source,
            })?;
        let stdin = child.stdin.take().expect("stdin is piped");
        let stdout = child.stdout.take().expect("stdout is piped");
        let stderr = child.stderr.take().expect("stderr is piped");
        let inbox = Arc::new(Inbox::default());
        inbox.lock().capacity = options.capacity;
        let reader_inbox = Arc::clone(&inbox);
        let reader = std::thread::Builder::new()
            .name(format!("tmux-observe-{session}"))
            .spawn(move || read_control_stream(stdout, stderr, &reader_inbox))
            .map_err(Error::io("start the reader thread for", "tmux".as_ref()))?;
        let observer = Observer {
            child,
            stdin,
            inbox,
            reader: Some(reader),
        };
        // Output a pane writes before tmux has attached the client never reaches it.
        let attach = observer
            .inbox
            .wait_until(deadline_after(ATTACH_TIMEOUT), |state| {
                state.attach.is_some()
            })
            .attach
            .clone();
        let failure = match attach {
            Some(Ok(())) => return Ok(observer),
            Some(Err(message)) => message,
            None => format!("no answer within {ATTACH_TIMEOUT:?}"),
        };
        Err(Error::CommandFailed {
            arguments: vec![
                "-C".into(),
                "attach-session".into(),
                session.target().into(),
            ],
            status: None,
            stderr: failure,
        })
    }
}

impl Observer {
    /// The next event, or `None` if none arrived within `timeout`. After the
    /// [`ControlEvent::Exit`] event, fails with [`Error::ObserverClosed`].
    pub fn next_event(&self, timeout: Duration) -> Result<Option<ControlEvent>, Error> {
        self.next_event_before(deadline_after(timeout))
    }

    /// [`Observer::next_event`] with a deadline (`None`: wait without limit).
    pub(crate) fn next_event_before(
        &self,
        deadline: Option<Instant>,
    ) -> Result<Option<ControlEvent>, Error> {
        let mut event = None;
        let state = self.inbox.wait_until(deadline, |state| {
            event = state.pop();
            event.is_some() || state.is_closed
        });
        match event {
            Some(event) => Ok(Some(event)),
            None if state.is_closed => Err(Error::ObserverClosed),
            None => Ok(None),
        }
    }

    /// Lets tmux send a paused pane's output again; tmux answers with a [`ControlEvent::Reply`].
    pub fn resume(&mut self, pane: &PaneId) -> Result<(), Error> {
        self.send(&[
            "refresh-client".into(),
            "-A".into(),
            format!("{pane}:continue").into(),
        ])
    }

    /// Sends one command line; its answer arrives as a [`ControlEvent::Reply`].
    pub(crate) fn send(&mut self, arguments: &[OsString]) -> Result<(), Error> {
        let mut line = Vec::new();
        for argument in arguments {
            line.extend(tmux_quoted(argument)?);
            line.push(b' ');
        }
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .and_then(|()| self.stdin.flush())
            .map_err(Error::io(
                "write a command to",
                "the tmux control client".as_ref(),
            ))
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // Killing the client closes its stdout, which ends the reader; it never waits on
        // the consumer, so joining it cannot hang.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Single-quotes a word for tmux's command parser, which, like `sh`, reads it literally.
/// The crate builds every command itself, so a quote or line break means a bug here.
fn tmux_quoted(word: &OsString) -> Result<Vec<u8>, Error> {
    use std::os::unix::ffi::OsStrExt;
    let bytes = word.as_bytes();
    if bytes
        .iter()
        .any(|byte| matches!(byte, b'\'' | b'\n' | b'\r'))
    {
        return Err(Error::InvalidArgument {
            what: "control-mode command word",
            value: word.to_string_lossy().into_owned(),
        });
    }
    let mut quoted = Vec::with_capacity(bytes.len() + 2);
    quoted.push(b'\'');
    quoted.extend_from_slice(bytes);
    quoted.push(b'\'');
    Ok(quoted)
}

fn read_control_stream(stdout: impl Read, mut stderr: impl Read, inbox: &Inbox) {
    let mut lines = BufReader::new(stdout);
    let mut parser = Parser::default();
    let mut has_exited = false;
    let mut read_error = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        match lines.read_until(b'\n', &mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error) => {
                read_error = Some(format!("reading the tmux control client failed: {error}"));
                break;
            }
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        match parser.feed(&line) {
            Some(Parsed::Attached(answer)) => inbox.update(|state| state.attach = Some(answer)),
            Some(Parsed::Event(event)) => {
                has_exited |= matches!(event, ControlEvent::Exit { .. });
                inbox.update(|state| state.push(event));
            }
            None => {}
        }
    }
    let mut message = String::new();
    let _ = stderr.read_to_string(&mut message);
    let reason = [read_error.unwrap_or_default(), message.trim().to_string()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    inbox.update(|state| {
        // tmux reports a missing session to `attach-session -C` only on stderr.
        state.attach.get_or_insert_with(|| Err(reason.clone()));
        if !has_exited {
            state.push(ControlEvent::Exit {
                reason: Some(reason).filter(|reason| !reason.is_empty()),
            });
        }
        state.is_closed = true;
    });
}

/// Turns control-mode lines into events. Lines between `%begin` and its `%end`/`%error` are
/// a command's output and are never read as notifications.
#[derive(Default)]
struct Parser {
    block: Option<Block>,
    has_attached: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Event(ControlEvent),
    /// tmux's answer to the `attach-session` the client was started with.
    Attached(Result<(), String>),
}

struct Block {
    /// `<time> <number> <flags>`, which the closing line repeats.
    guard: Vec<u8>,
    /// Flags 1: this client sent the command. tmux also answers the `attach-session` it was
    /// started with (flags 0); that answer is not a [`ControlEvent::Reply`].
    is_from_this_client: bool,
    lines: Vec<Vec<u8>>,
}

impl Parser {
    fn feed(&mut self, line: &[u8]) -> Option<Parsed> {
        if let Some(block) = &mut self.block {
            let closing = [&b"%end "[..], b"%error "]
                .into_iter()
                .find(|prefix| line.strip_prefix(*prefix) == Some(&block.guard[..]));
            let Some(closing) = closing else {
                block.lines.push(line.to_vec());
                return None;
            };
            let block = self.block.take().expect("a block is open");
            let is_error = closing == b"%error ";
            if !block.is_from_this_client {
                let message = String::from_utf8_lossy(&block.lines.join(&b'\n')).into_owned();
                if !self.has_attached {
                    self.has_attached = true;
                    return Some(Parsed::Attached(if is_error {
                        Err(message)
                    } else {
                        Ok(())
                    }));
                }
                return is_error.then_some(Parsed::Event(ControlEvent::Notification {
                    name: "error".to_string(),
                    arguments: message,
                }));
            }
            return Some(Parsed::Event(ControlEvent::Reply {
                is_error,
                lines: block.lines,
            }));
        }
        if let Some(guard) = line.strip_prefix(b"%begin ") {
            self.block = Some(Block {
                guard: guard.to_vec(),
                is_from_this_client: guard.ends_with(b" 1"),
                lines: Vec::new(),
            });
            return None;
        }
        parse_notification(line).map(Parsed::Event)
    }
}

fn parse_notification(line: &[u8]) -> Option<ControlEvent> {
    let line = line.strip_prefix(b"%")?;
    let (name, arguments) = match line.iter().position(|&byte| byte == b' ') {
        Some(space) => (&line[..space], &line[space + 1..]),
        None => (line, &b""[..]),
    };
    let pane_argument = || {
        let pane = arguments.split(|&byte| byte == b' ').next()?;
        PaneId::parse(std::str::from_utf8(pane).ok()?).ok()
    };
    let event = match name {
        b"output" => {
            let space = arguments.iter().position(|&byte| byte == b' ')?;
            ControlEvent::Output {
                pane: pane_argument()?,
                bytes: unescape_output(&arguments[space + 1..]),
            }
        }
        b"extended-output" => {
            let separator = arguments.windows(3).position(|window| window == b" : ")?;
            ControlEvent::Output {
                pane: pane_argument()?,
                bytes: unescape_output(&arguments[separator + 3..]),
            }
        }
        b"pause" => ControlEvent::OutputLost {
            pane: pane_argument()?,
            is_paused: true,
        },
        b"exit" => ControlEvent::Exit {
            reason: Some(String::from_utf8_lossy(arguments).into_owned())
                .filter(|reason| !reason.is_empty()),
        },
        _ => ControlEvent::Notification {
            name: String::from_utf8_lossy(name).into_owned(),
            arguments: String::from_utf8_lossy(arguments).into_owned(),
        },
    };
    Some(event)
}

/// tmux writes a pane's bytes below 32 and the backslash as `\` and three octal digits.
fn unescape_output(escaped: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(escaped.len());
    let mut index = 0;
    while index < escaped.len() {
        let octal = escaped.get(index + 1..index + 4).filter(|digits| {
            escaped[index] == b'\\' && digits.iter().all(|digit| (b'0'..=b'7').contains(digit))
        });
        match octal {
            Some(digits) => {
                let value = digits
                    .iter()
                    .fold(0u32, |value, digit| value * 8 + u32::from(digit - b'0'));
                bytes.push(u8::try_from(value).unwrap_or(u8::MAX));
                index += 4;
            }
            None => {
                bytes.push(escaped[index]);
                index += 1;
            }
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_escapes_become_bytes_and_other_bytes_pass_through() {
        assert_eq!(unescape_output(br"a\015\012\134b\\"), b"a\r\n\\b\\\\");
    }

    #[test]
    fn a_reply_line_that_looks_like_a_notification_stays_in_the_reply() {
        let mut parser = Parser::default();
        assert_eq!(parser.feed(b"%begin 1 7 1"), None);
        assert_eq!(parser.feed(b"%output %3 not a notification"), None);
        assert_eq!(parser.feed(b"%end 1 6 1"), None);
        assert_eq!(
            parser.feed(b"%end 1 7 1"),
            Some(Parsed::Event(ControlEvent::Reply {
                is_error: false,
                lines: vec![
                    b"%output %3 not a notification".to_vec(),
                    b"%end 1 6 1".to_vec()
                ],
            }))
        );
    }
}
