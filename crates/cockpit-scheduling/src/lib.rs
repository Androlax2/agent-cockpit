#![doc = include_str!("../README.md")]

mod error;
mod model;
mod pacing;
mod plan;
mod store;

pub use error::Error;
pub use model::{
    ActionBy, Candidate, Dispatched, Explanation, Inputs, Placement, PlacementId, Presence, Reason,
    Role, SessionNote, Settings, Unit,
};
pub use pacing::{Pacing, ProjectShare, Standing, UsageWindow, WindowPace, window_pace};
pub use store::{
    DELIVERY_PRODUCER, Handoff, MIGRATIONS, active_placement, awaits_pass, delivery_failed,
    delivery_key, dispatch, explanations, handoff, placement_of_delivery, placements,
};
