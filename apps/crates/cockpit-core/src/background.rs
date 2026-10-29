//! Coalesced notification after a background worker publishes a result.

use std::sync::atomic::{AtomicBool, Ordering};

static BACKGROUND_CHANGED: AtomicBool = AtomicBool::new(false);

/// Called explicitly after publishing a worker result; the terminal consumes it without cloning
/// provider summaries or review lists on each input wait.
pub fn background_changed() {
    BACKGROUND_CHANGED.store(true, Ordering::Release);
}

pub fn take_background_changed() -> bool {
    BACKGROUND_CHANGED.swap(false, Ordering::Acquire)
}
