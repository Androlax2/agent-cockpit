//! Headless session registry and collaboration engine used by the OSD presenter.

pub mod chief;
pub mod fifo;
pub mod load;
pub mod registry;
pub mod teams;

#[cfg(test)]
mod tests;
