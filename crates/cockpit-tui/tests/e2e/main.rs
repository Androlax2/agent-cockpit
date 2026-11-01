//! The TUI end to end (#300): the real daemon, the real `cockpit tui` in a terminal, a fake
//! Claude Code, keys pressed and screens read, the way Playwright drives a browser. Each
//! module is one section of a live acceptance guide (`docs/acceptance/`); a finding from a
//! live walk gets a failing test here before its fix.
//!
//! tmux must be installed: the daemon hosts the TUI and every agent in it (report 05).

mod harness;

mod attach;
mod lifecycle;
mod list;
mod new_session;

// These need the agent channel, which reads process trees on Linux only (#336).
#[cfg(target_os = "linux")]
mod ask;
#[cfg(target_os = "linux")]
mod idle;
#[cfg(target_os = "linux")]
mod needs_you;
