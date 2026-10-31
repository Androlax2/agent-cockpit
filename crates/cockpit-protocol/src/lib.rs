#![doc = include_str!("../README.md")]

mod change_feed;
mod error;
mod handshake;
mod request;

pub use change_feed::{
    Change, ChangeBatch, ChangeEvent, CommitRevision, Cursor, DatabaseEpoch, EntityKind, EntityRef,
    EntityRevision, FeedPosition,
};
pub use error::{ErrorCode, ProtocolError};
pub use handshake::{
    Capability, HandshakeReply, Hello, ProtocolVersion, Refusal, RefusalReason, SUPPORTED_VERSIONS,
    Welcome, decode_hello, negotiate,
};
pub use request::{
    Command, Outcome, Reply, Request, RequestId, RequestRejection, Response, ServerFrame,
    decode_request,
};
