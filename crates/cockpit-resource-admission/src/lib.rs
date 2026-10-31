#![doc = include_str!("../README.md")]

mod backend;
mod error;
mod model;
mod store;

pub use backend::{
    Backend, Effective, ExecutionReport, HostTools, LimitReport, Observed, Platform,
};
pub use error::Error;
pub use model::{
    Admission, AdmissionRequest, Availability, Capacity, Control, ControlSupport, Demand, Hold,
    InUse, Limits, Policy, Profile, ReleaseReason, Reservation, ReservationId, ReservationState,
    Resource, Settings, Swept, Verdict, Workload,
};
pub use store::{
    MIGRATIONS, admit, bind_execution, check, request_checkpoint, reservation, reservations,
    set_profile, set_queue_paused, settings, sweep, withdraw,
};
