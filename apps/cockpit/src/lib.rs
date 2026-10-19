//! The Claude cockpit: the code its programs share (paths, sessions, usage, the column's app).

pub mod app;
pub mod approval;
pub mod audit;
pub mod costs;
pub mod errors;
pub mod paths;
pub mod prompts;
pub mod remote;
pub mod reviews;
pub mod sessions;
pub mod settings;
pub mod status;
pub mod todo;
pub mod usage;
pub mod util;

#[cfg(test)]
mod tests;
