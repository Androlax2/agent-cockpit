//! The capture stream clients render a session from: a snapshot of each pane's screen, then
//! its output as it arrives, and a fresh snapshot whenever flow control dropped output.
//! A slow client therefore costs a re-capture, never an unbounded backlog.

use std::collections::{HashSet, VecDeque};
use std::time::{Duration, Instant};

use crate::Error;
use crate::control::{ControlEvent, ObserveOptions, Observer};
use crate::names::{PaneId, SessionName};
use crate::server::{Server, capture_arguments};

/// What a client draws. All bytes are untrusted display data (report 05).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureFrame {
    /// The pane's whole screen, as [`Server::capture`] returns it: replace what was shown.
    Snapshot { pane: PaneId, screen: Vec<u8> },
    /// Bytes the pane's program wrote after the last snapshot of that pane.
    Output { pane: PaneId, bytes: Vec<u8> },
    /// tmux could not capture or resume this pane, usually because it closed; the stream
    /// goes on for the session's other panes.
    PaneUnavailable { pane: PaneId, message: String },
    /// The session closed or the server stopped; the stream has nothing more.
    Ended { reason: Option<String> },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureOptions {
    pub observe: ObserveOptions,
    /// Lines of history above the visible screen in each snapshot.
    pub history_lines: u32,
}

pub struct CaptureStream {
    observer: Observer,
    /// The commands sent and not yet answered, in sending order.
    awaiting_replies: VecDeque<Request>,
    /// Panes that lost output while a snapshot of them was already requested: that snapshot
    /// may predate the loss, so another follows it. At most two snapshots of a pane are
    /// ever outstanding, however long the client stays behind.
    needs_another_snapshot: HashSet<PaneId>,
    history_lines: u32,
}

#[derive(Debug)]
enum Request {
    Snapshot(PaneId),
    Resume(PaneId),
}

impl Server {
    /// Observes `session` and asks for a snapshot of each of its panes. The snapshot is taken
    /// by the observing client itself, so it sits in the same ordered stream as the output.
    pub fn capture_stream(
        &self,
        session: &SessionName,
        options: CaptureOptions,
    ) -> Result<CaptureStream, Error> {
        let panes = self.panes(session)?;
        let observer = self.observe_listed_session(session, options.observe)?;
        let mut stream = CaptureStream {
            observer,
            awaiting_replies: VecDeque::new(),
            needs_another_snapshot: HashSet::new(),
            history_lines: options.history_lines,
        };
        for pane in panes {
            stream.request_snapshot(pane.id)?;
        }
        Ok(stream)
    }
}

impl CaptureStream {
    /// The next frame, or `None` if none is ready within `timeout`.
    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<CaptureFrame>, Error> {
        let deadline = Instant::now().checked_add(timeout);
        loop {
            let remaining = deadline.map_or(Duration::MAX, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });
            let Some(event) = self.observer.next_event(remaining)? else {
                return Ok(None);
            };
            match event {
                ControlEvent::Output { pane, bytes } => {
                    return Ok(Some(CaptureFrame::Output { pane, bytes }));
                }
                ControlEvent::OutputLost { pane, is_paused } => {
                    if is_paused {
                        self.observer.resume(&pane)?;
                        self.awaiting_replies
                            .push_back(Request::Resume(pane.clone()));
                    }
                    if self.is_snapshot_pending(&pane) {
                        self.needs_another_snapshot.insert(pane);
                    } else {
                        self.request_snapshot(pane)?;
                    }
                }
                ControlEvent::Reply { is_error, lines } => {
                    if let Some(frame) = self.frame_for_reply(is_error, lines)? {
                        return Ok(Some(frame));
                    }
                }
                ControlEvent::Exit { reason } => return Ok(Some(CaptureFrame::Ended { reason })),
                ControlEvent::Notification { .. } => {}
            }
        }
    }

    fn is_snapshot_pending(&self, pane: &PaneId) -> bool {
        self.awaiting_replies
            .iter()
            .any(|request| matches!(request, Request::Snapshot(pending) if pending == pane))
    }

    fn request_snapshot(&mut self, pane: PaneId) -> Result<(), Error> {
        self.observer
            .send(&capture_arguments(&pane, self.history_lines))?;
        self.awaiting_replies.push_back(Request::Snapshot(pane));
        Ok(())
    }

    fn frame_for_reply(
        &mut self,
        is_error: bool,
        lines: Vec<Vec<u8>>,
    ) -> Result<Option<CaptureFrame>, Error> {
        let Some(request) = self.awaiting_replies.pop_front() else {
            return Err(Error::UnexpectedOutput {
                arguments: Vec::new(),
                output: "a control-mode reply to no command this stream sent".to_string(),
            });
        };
        let (Request::Snapshot(pane) | Request::Resume(pane)) = &request;
        let pane = pane.clone();
        if is_error {
            // Usually the pane closed after the request; the other panes stream on.
            self.needs_another_snapshot.remove(&pane);
            return Ok(Some(CaptureFrame::PaneUnavailable {
                pane,
                message: String::from_utf8_lossy(&lines.join(&b'\n')).into_owned(),
            }));
        }
        let Request::Snapshot(_) = request else {
            return Ok(None);
        };
        if self.needs_another_snapshot.remove(&pane) {
            self.request_snapshot(pane.clone())?;
        }
        let mut screen = lines.join(&b'\n');
        screen.push(b'\n');
        Ok(Some(CaptureFrame::Snapshot { pane, screen }))
    }
}
