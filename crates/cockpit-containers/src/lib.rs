#![doc = include_str!("../README.md")]

mod info;

pub use info::{DOCKER_DEADLINE, EngineInfo, probe_docker};
