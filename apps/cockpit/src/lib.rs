//! Cockpit terminal application, executable support and compatibility exports.

pub use cockpit_core::agents;
pub use cockpit_core::audit;
pub use cockpit_core::background;
pub use cockpit_core::billing;
pub use cockpit_core::codex;
pub use cockpit_core::continuity;
pub use cockpit_core::costs;
pub use cockpit_core::errors;
pub use cockpit_core::failsafe;
pub use cockpit_core::paths;
pub use cockpit_core::prompts;
pub use cockpit_core::push;
pub use cockpit_core::queue;
pub use cockpit_core::remote;
pub use cockpit_core::reviews;
pub use cockpit_core::sessions;
pub use cockpit_core::settings;
pub use cockpit_core::sleep;
pub use cockpit_core::status;
pub use cockpit_core::subscription_fallback;
pub use cockpit_core::todo;
pub use cockpit_core::todo_images;
pub use cockpit_core::todo_requests;
pub use cockpit_core::util;

pub mod app;
pub mod approval;
pub mod chief;
pub mod teams;
pub mod usage;

mod todo_composer;
mod todo_editor;

#[cfg(test)]
mod tests;
