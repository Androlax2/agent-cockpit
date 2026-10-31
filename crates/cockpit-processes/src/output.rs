//! Bounded draining of an execution's output files into a live stream.
//!
//! The program writes to files, not pipes: a pipe's reader is the daemon, and a write after a
//! daemon restart would kill the program with `SIGPIPE` (docs/DECISIONS.md). The daemon
//! drains each file in bounded chunks into a byte-bounded window. A viewer that falls behind
//! the window is told exactly which bytes it missed (a gap marker) and continues from the
//! oldest byte still held; live output is disposable (report 03).

use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::sync::watch;

use crate::Error;

/// Which of an execution's output files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    Stdout,
    Stderr,
}

impl OutputKind {
    pub(crate) fn file_name(self) -> &'static str {
        match self {
            OutputKind::Stdout => "stdout",
            OutputKind::Stderr => "stderr",
        }
    }
}

/// How much output the daemon holds and reads at a time.
#[derive(Debug, Clone, Copy)]
pub struct OutputLimits {
    /// The most bytes one stream keeps for viewers; older bytes become a gap.
    pub window_bytes: usize,
    /// The most bytes one drain step reads from the file.
    pub chunk_bytes: usize,
    /// How long the drainer waits before looking for new bytes when the file had none.
    pub poll_interval: Duration,
}

impl Default for OutputLimits {
    fn default() -> OutputLimits {
        OutputLimits {
            window_bytes: 1024 * 1024,
            chunk_bytes: 64 * 1024,
            poll_interval: Duration::from_millis(50),
        }
    }
}

/// What a viewer reads at its cursor (a byte offset into the file).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputRead {
    /// Bytes starting at `offset`; the next cursor is `offset + bytes.len()`.
    Bytes { offset: u64, bytes: Vec<u8> },
    /// The bytes `from..to` left the window before this viewer read them; continue at `to`.
    Gap { from: u64, to: u64 },
    /// The execution ended and the viewer has read everything that was drained.
    Finished,
    /// Draining stopped on an error after everything before it was read; the file itself
    /// still holds the rest.
    Failed { reason: String },
}

#[derive(Debug)]
struct Window {
    /// File offset of `bytes[0]`.
    start: u64,
    bytes: VecDeque<u8>,
    capacity: usize,
    finished: bool,
    failure: Option<String>,
}

impl Window {
    fn end(&self) -> u64 {
        self.start + self.bytes.len() as u64
    }

    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk);
        let excess = self.bytes.len().saturating_sub(self.capacity);
        self.bytes.drain(..excess);
        self.start += excess as u64;
    }

    fn read(&self, cursor: u64, max_bytes: usize) -> Option<OutputRead> {
        if cursor < self.start {
            return Some(OutputRead::Gap {
                from: cursor,
                to: self.start,
            });
        }
        if cursor < self.end() {
            let skip = usize::try_from(cursor - self.start).expect("inside the window");
            let bytes: Vec<u8> = self
                .bytes
                .iter()
                .skip(skip)
                .take(max_bytes)
                .copied()
                .collect();
            return Some(OutputRead::Bytes {
                offset: cursor,
                bytes,
            });
        }
        if let Some(reason) = &self.failure {
            return Some(OutputRead::Failed {
                reason: reason.clone(),
            });
        }
        self.finished.then_some(OutputRead::Finished)
    }
}

/// One output file followed by a drainer task. Cloning shares the stream.
#[derive(Debug, Clone)]
pub struct OutputStream {
    window: Arc<Mutex<Window>>,
    /// The drained end offset, and whether the drainer finished; viewers wait on it.
    progress: watch::Receiver<(u64, bool)>,
}

impl OutputStream {
    /// Starts draining `path` from its beginning until `ended` resolves and the file has no
    /// more bytes. `ended` resolves once the execution was observed empty: after that no
    /// process can append, so the last drain is complete.
    pub(crate) fn follow(
        path: PathBuf,
        limits: OutputLimits,
        ended: impl Future<Output = ()> + Send + 'static,
    ) -> OutputStream {
        let window = Arc::new(Mutex::new(Window {
            start: 0,
            bytes: VecDeque::with_capacity(limits.window_bytes.min(limits.chunk_bytes)),
            capacity: limits.window_bytes,
            finished: false,
            failure: None,
        }));
        let (progress_sender, progress) = watch::channel((0, false));
        tokio::spawn(drain(
            path,
            limits,
            Arc::clone(&window),
            progress_sender,
            ended,
        ));
        OutputStream { window, progress }
    }

    /// Reads at `cursor` at most `max_bytes`, waiting until there is something to read.
    pub async fn read(&self, cursor: u64, max_bytes: usize) -> OutputRead {
        let mut progress = self.progress.clone();
        loop {
            if let Some(read) = self
                .window
                .lock()
                .expect("the window lock is never held across a panic")
                .read(cursor, max_bytes)
            {
                return read;
            }
            if progress.changed().await.is_err() {
                return OutputRead::Failed {
                    reason: "the drainer stopped without finishing".to_owned(),
                };
            }
        }
    }

    /// The bytes the window holds now; never more than `window_bytes`.
    pub fn held_bytes(&self) -> usize {
        self.window
            .lock()
            .expect("the window lock is never held across a panic")
            .bytes
            .len()
    }
}

async fn drain(
    path: PathBuf,
    limits: OutputLimits,
    window: Arc<Mutex<Window>>,
    progress: watch::Sender<(u64, bool)>,
    ended: impl Future<Output = ()> + Send + 'static,
) {
    let drained = drain_until_ended(&path, limits, &window, &progress, ended).await;
    let mut window = window
        .lock()
        .expect("the window lock is never held across a panic");
    window.finished = true;
    window.failure = drained.err().map(|error| error.to_string());
    let end = window.end();
    progress.send_replace((end, true));
}

async fn drain_until_ended(
    path: &std::path::Path,
    limits: OutputLimits,
    window: &Mutex<Window>,
    progress: &watch::Sender<(u64, bool)>,
    ended: impl Future<Output = ()>,
) -> Result<(), Error> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(Error::io("open", path))?;
    let mut chunk = vec![0; limits.chunk_bytes];
    let mut drained: u64 = 0;
    let mut ended = std::pin::pin!(ended);
    let mut has_ended = false;
    loop {
        file.seek(std::io::SeekFrom::Start(drained))
            .await
            .map_err(Error::io("seek", path))?;
        let read = file
            .read(&mut chunk)
            .await
            .map_err(Error::io("read", path))?;
        if read > 0 {
            drained += read as u64;
            window
                .lock()
                .expect("the window lock is never held across a panic")
                .push(&chunk[..read]);
            progress.send_replace((drained, false));
            continue;
        }
        if has_ended {
            return Ok(());
        }
        tokio::select! {
            () = &mut ended => has_ended = true,
            () = tokio::time::sleep(limits.poll_interval) => {}
        }
    }
}
