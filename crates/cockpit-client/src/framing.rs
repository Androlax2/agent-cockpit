use serde::Serialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

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

/// Reads the next frame from `reader` into `frame`, without its newline, or says why the
/// stream cannot yield one. `frame` is cleared first; reusing it keeps its capacity.
pub(crate) async fn next_frame<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    frame: &mut Vec<u8>,
) -> Result<(), CloseReason> {
    frame.clear();
    let read = reader
        .take(MAX_FRAME_BYTES as u64)
        .read_until(b'\n', frame)
        .await
        .map_err(|error| CloseReason::Io(error.to_string()))?;
    match frame.pop() {
        Some(b'\n') => Ok(()),
        None => Err(CloseReason::Disconnected),
        Some(_) if read == MAX_FRAME_BYTES => Err(CloseReason::FrameTooLarge),
        Some(_) => Err(CloseReason::Truncated),
    }
}
