#![doc = include_str!("../README.md")]

mod board;
mod event;
mod feed;
mod notifier;
mod outlets;

pub use board::FeedError;
pub use event::{Event, names};
pub use feed::{FeedEvent, follow_feed};
pub use notifier::{Notice, Notifier};
pub use outlets::{Desktop, Host, Outlets, Sound, sound_id};
