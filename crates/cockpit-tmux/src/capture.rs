//! The capture stream clients render a session from: a snapshot of each pane's screen, then
//! its output as it arrives, and a fresh snapshot whenever flow control dropped output.
//! A slow client therefore costs a re-capture, never an unbounded backlog.

use std::collections::VecDeque;
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
    /// One entry per command sent and not yet answered, in sending order: the pane a
    /// capture was for, or `None` for a resume.
    awaiting_replies: VecDeque<Option<PaneId>>,
    history_lines: u32,
}

impl Server {
    /// Observes `session` and asks for a snapshot of each of its panes. The snapshot is taken
    /// by the observing client itself, so it sits in the same ordered stream as the output.
    pub fn capture_stream(
        &self,
        session: &SessionName,
        options: CaptureOptions,
    ) -> Result<CaptureStream, Error> {
        let observer = self.observe(session, options.observe)?;
        let mut stream = CaptureStream {
            observer,
            awaiting_replies: VecDeque::new(),
            history_lines: options.history_lines,
        };
        for pane in self.panes(session)? {
            stream.request_snapshot(pane.id)?;
        }
        Ok(stream)
    }
}

impl CaptureStream {
    /// The next frame, or `None` if none is ready within `timeout`.
    pub fn next_frame(&mut self, timeout: Duration) -> Result<Option<CaptureFrame>, Error> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
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
                        self.awaiting_replies.push_back(None);
                    }
                    self.request_snapshot(pane)?;
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

    fn request_snapshot(&mut self, pane: PaneId) -> Result<(), Error> {
        self.observer
            .send(&capture_arguments(&pane, self.history_lines))?;
        self.awaiting_replies.push_back(Some(pane));
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
        if is_error {
            let arguments = match &request {
                Some(pane) => capture_arguments(pane, self.history_lines),
                None => vec!["refresh-client".into(), "-A".into()],
            };
            return Err(Error::CommandFailed {
                arguments,
                status: None,
                stderr: String::from_utf8_lossy(&lines.join(&b'\n')).into_owned(),
            });
        }
        Ok(request.map(|pane| {
            let mut screen = lines.join(&b'\n');
            screen.push(b'\n');
            CaptureFrame::Snapshot { pane, screen }
        }))
    }
}
