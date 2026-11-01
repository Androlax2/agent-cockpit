#![doc = include_str!("../README.md")]

mod capture;
mod control;
mod error;
mod names;
mod server;
mod version;

pub use capture::{CaptureFrame, CaptureOptions, CaptureStream};
pub use control::{ControlEvent, ObserveOptions, Observer};
pub use error::Error;
pub use names::{PaneId, SessionName};
pub use server::{
    CONFIG, Keystroke, PaneInfo, PaneState, Server, ServerSpec, SessionLaunch, Socket, Split,
};
pub use version::{MINIMUM_VERSION, TmuxVersion, check_version};
