//! The cockpit's GTK programs: the OSD daemon (volume and brightness island, the Claude banners and
//! the session registry, the run and guide queues, the teams) and the idle screen.

pub mod chief;
pub mod fifo;
pub mod load;
pub mod registry;
pub mod teams;

#[cfg(test)]
mod tests;
