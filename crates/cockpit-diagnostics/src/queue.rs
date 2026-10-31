//! The non-blocking writer: callers hand whole records to a bounded queue and never wait;
//! one thread owns the file. A full queue or a failed write drops the record and counts it.

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::{FormatTime, SystemTime};

use crate::Error;
use crate::rotation::RotatingFile;

/// How often an idle writer checks whether shutdown began.
const IDLE_POLL: Duration = Duration::from_millis(50);
/// Removes expired rotated files even when a quiet process never rotates.
const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);

pub(crate) struct Queue {
    writer: QueueWriter,
    closing: Arc<AtomicBool>,
    /// Set by the writer thread once everything queued before shutdown reached the file.
    drained: Arc<(Mutex<bool>, Condvar)>,
}

/// The [`MakeWriter`] the layers write through; cheap to clone.
#[derive(Debug, Clone)]
pub(crate) struct QueueWriter {
    sender: SyncSender<Vec<u8>>,
    dropped: Arc<AtomicU64>,
}

impl Queue {
    pub(crate) fn start(file: RotatingFile, capacity: usize) -> Result<Self, Error> {
        if capacity == 0 {
            return Err(Error::InvalidLimits {
                reason: "queue_records must be at least 1",
            });
        }
        let (sender, receiver) = mpsc::sync_channel(capacity);
        let writer = QueueWriter {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
        };
        let closing = Arc::new(AtomicBool::new(false));
        let drained = Arc::new((Mutex::new(false), Condvar::new()));
        let worker = Worker {
            file,
            receiver,
            dropped: Arc::clone(&writer.dropped),
            reported: 0,
            capacity,
            closing: Arc::clone(&closing),
            drained: Arc::clone(&drained),
        };
        thread::Builder::new()
            .name(String::from("cockpit-log-writer"))
            .spawn(move || worker.run())
            .map_err(|source| Error::Io {
                action: "start the log writer for",
                path: "cockpit.log".into(),
                source,
            })?;
        Ok(Self {
            writer,
            closing,
            drained,
        })
    }

    pub(crate) fn writer(&self) -> QueueWriter {
        self.writer.clone()
    }

    pub(crate) fn dropped_records(&self) -> u64 {
        self.writer.dropped.load(Ordering::Relaxed)
    }

    pub(crate) fn shutdown(&self, timeout: Duration) -> bool {
        self.closing.store(true, Ordering::Relaxed);
        let (is_drained, changed) = &*self.drained;
        let is_drained = is_drained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (is_drained, _) = changed
            .wait_timeout_while(is_drained, timeout, |is_drained| !*is_drained)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *is_drained
    }
}

impl QueueWriter {
    /// Queues one whole record without waiting.
    pub(crate) fn push(&self, record: Vec<u8>) {
        match self.sender.try_send(record) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

impl<'a> MakeWriter<'a> for QueueWriter {
    type Writer = RecordWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        RecordWriter {
            queue: self,
            record: Vec::new(),
        }
    }
}

/// Collects one formatted record and queues it whole when dropped, so records never interleave.
pub(crate) struct RecordWriter<'a> {
    queue: &'a QueueWriter,
    record: Vec<u8>,
}

impl io::Write for RecordWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.record.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for RecordWriter<'_> {
    fn drop(&mut self) {
        if !self.record.is_empty() {
            self.queue.push(std::mem::take(&mut self.record));
        }
    }
}

struct Worker {
    file: RotatingFile,
    receiver: Receiver<Vec<u8>>,
    dropped: Arc<AtomicU64>,
    /// Drops already announced in the log.
    reported: u64,
    /// Bounds the shutdown drain to what the queue held, while producers keep logging.
    capacity: usize,
    closing: Arc<AtomicBool>,
    drained: Arc<(Mutex<bool>, Condvar)>,
}

impl Worker {
    fn run(mut self) {
        let mut last_pruned = Instant::now();
        while !self.closing.load(Ordering::Relaxed) {
            match self.receiver.recv_timeout(IDLE_POLL) {
                Ok(record) => self.write(&record),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if last_pruned.elapsed() >= PRUNE_INTERVAL {
                self.file.prune();
                last_pruned = Instant::now();
            }
        }
        let backlog: Vec<Vec<u8>> = self.receiver.try_iter().take(self.capacity).collect();
        for record in backlog {
            self.write(&record);
        }
        self.report_drops();
        self.file.flush();
        // Later records find the queue disconnected and are counted as dropped.
        drop(self.receiver);
        let (is_drained, changed) = &*self.drained;
        *is_drained
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
        changed.notify_all();
    }

    fn write(&mut self, record: &[u8]) {
        self.report_drops();
        if !self.file.write_record(record) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Writes a `warn` record for drops not yet announced; it says how many were lost.
    fn report_drops(&mut self) {
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped == self.reported {
            return;
        }
        let report = own_record(
            "WARN",
            "dropped log records",
            [("dropped", (dropped - self.reported).into())],
        );
        if self.file.write_record(&report) {
            self.reported = dropped;
        }
    }
}

/// A record written by this crate itself, shaped like the JSON layer's records.
pub(crate) fn own_record<const N: usize>(
    level: &str,
    message: &str,
    fields: [(&str, serde_json::Value); N],
) -> Vec<u8> {
    let mut timestamp = String::new();
    // Formatting the system time into a String cannot fail.
    let _ = SystemTime.format_time(&mut Writer::new(&mut timestamp));
    let mut record_fields = serde_json::Map::new();
    record_fields.insert(String::from("message"), message.into());
    for (name, value) in fields {
        record_fields.insert(name.to_owned(), value);
    }
    let record = serde_json::json!({
        "timestamp": timestamp,
        "level": level,
        "fields": record_fields,
        "target": "cockpit_diagnostics",
    });
    let mut line = record.to_string().into_bytes();
    line.push(b'\n');
    line
}
