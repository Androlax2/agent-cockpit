//! The Claude cockpit: the code its programs share (paths, sessions, usage, the column's app).

pub mod agents;
pub mod app;
pub mod approval;
pub mod audit;
pub mod costs;
pub mod errors;
pub mod paths;
pub mod prompts;
pub mod push;
pub mod remote;
pub mod reviews;
pub mod sessions;
pub mod settings;
pub mod status;
pub mod todo;
mod todo_editor;
mod todo_composer;
pub mod usage;
pub mod util;

#[cfg(test)]
mod tests;
