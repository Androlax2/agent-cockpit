use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};

use crate::error::{ClientError, CloseReason};

/// Longest frame either side may send, newline included. Large enough for a snapshot of a
/// personal installation's state, small enough that a runaway peer cannot exhaust memory.
pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// `frame` as one line of JSON, refused when it would exceed [`MAX_FRAME_BYTES`].
pub(crate) fn encode(frame: &impl Serialize) -> Result<Vec<u8>, ClientError> {
    let mut line = serde_json::to_vec(frame).map_err(ClientError::Encode)?;
    line.push(b'\n');
    if line.len() > MAX_FRAME_BYTES {
        return Err(ClientError::FrameTooLarge);
    }
    Ok(line)
}

/// Reads newline-delimited frames, never buffering more than [`MAX_FRAME_BYTES`] of one.
pub(crate) struct FrameReader<R> {
    reader: BufReader<R>,
    partial_frame: Vec<u8>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            partial_frame: Vec::new(),
        }
    }

    /// The next frame without its newline, or the reason the stream cannot yield one.
    pub(crate) async fn next_frame(&mut self) -> Result<Vec<u8>, CloseReason> {
        loop {
            let available = self
                .reader
                .fill_buf()
                .await
                .map_err(|error| CloseReason::Io(error.to_string()))?;
            if available.is_empty() {
                return Err(if self.partial_frame.is_empty() {
                    CloseReason::Disconnected
                } else {
                    CloseReason::Truncated
                });
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let frame_bytes = newline.unwrap_or(available.len());
            if self.partial_frame.len() + frame_bytes + 1 > MAX_FRAME_BYTES {
                return Err(CloseReason::FrameTooLarge);
            }
            self.partial_frame
                .extend_from_slice(&available[..frame_bytes]);
            let consumed = newline.map_or(frame_bytes, |_| frame_bytes + 1);
            self.reader.consume(consumed);
            if newline.is_some() {
                return Ok(std::mem::take(&mut self.partial_frame));
            }
        }
    }
}
