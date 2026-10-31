#![doc = include_str!("../README.md")]

mod connection;
mod error;
mod framing;
mod subscription;

pub use connection::Connection;
pub use error::{ClientError, CloseReason};
pub use framing::MAX_FRAME_BYTES;
pub use subscription::{PendingBatch, Subscription};
