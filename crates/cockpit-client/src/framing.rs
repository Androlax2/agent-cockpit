use serde::Serialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

use crate::error::{ClientError, CloseReason};

pub use cockpit_protocol::MAX_FRAME_BYTES;

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
///
/// Not cancel safe: bytes of a partly read frame are lost. Callers read from one task and
/// drop the whole stream when they stop.
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
