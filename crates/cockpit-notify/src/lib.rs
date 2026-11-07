#![doc = include_str!("../README.md")]

mod board;
mod bundled;
mod event;
mod feed;
mod hold;
mod notifier;
mod outlets;
mod status;

pub use board::FeedError;
pub use bundled::install as install_bundled_sounds;
pub use event::{Event, names};
pub use feed::{FeedEvent, follow_feed};
pub use hold::{AWAY, Detector, Detectors, Hold, standing_holds};
pub use notifier::{Notice, Notifier};
pub use outlets::{Desktop, Host, Outlets, Sound, sound_id};
pub use status::{STATUS_SCHEMA, SessionState, SessionStatus, Status, write_status};
