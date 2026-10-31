//! Cockpit's published wire contract: what clients, the daemon and future external plugins
//! exchange (docs/design/greenfield/14-plugins.md treats it as stable from the first release).
//!
//! These types are the serialized form only. Capabilities keep their own internal models and
//! convert at the boundary, so a field rename inside a capability never changes the wire.
//!
//! Forward compatibility: unknown object fields are ignored, and enums whose set grows over
//! time (capabilities, reasons, error codes, entity kinds) keep a value they do not know in
//! an `Unknown` variant instead of failing. An unknown command kind reads as
//! `Command::Unknown`, which `decode_request` refuses as `unknown-command`.

mod change_feed;
mod error;
mod handshake;
mod request;

pub use change_feed::{
    Change, ChangeBatch, ChangeEvent, CommitRevision, Cursor, DatabaseEpoch, EntityKind, EntityRef,
    EntityRevision, Follows,
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
