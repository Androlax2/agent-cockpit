//! The cockpit's GTK programs: the OSD daemon (volume and brightness island, the Claude banners and
//! the session registry, the run and guide queues, the teams) and the idle screen.

pub mod fifo;
pub mod registry;
pub mod teams;

#[cfg(test)]
mod tests;
