//! The Agent Cockpit: the code its programs share (paths, sessions, usage, the column's app).

pub mod agents;
pub mod app;
pub mod approval;
pub mod audit;
pub mod billing;
pub mod chief;
pub mod codex;
pub mod continuity;
pub mod costs;
pub mod errors;
pub mod failsafe;
pub mod game_guard;
pub mod paths;
pub mod prompts;
pub mod push;
pub mod queue;
pub mod remote;
pub mod reviews;
pub mod sessions;
pub mod settings;
pub mod sleep;
pub mod status;
pub mod subscription_fallback;
pub mod teams;
pub mod todo;
mod todo_composer;
mod todo_editor;
pub mod todo_images;
pub mod todo_requests;
pub mod usage;
pub mod util;

#[cfg(test)]
mod tests;
