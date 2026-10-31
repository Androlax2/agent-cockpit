//! Bounded admission: each lane has a fixed number of workers and of waiting places. A full
//! lane answers `busy` at once; it never queues without bound (report 03).

use std::sync::Arc;

use cockpit_protocol::{ErrorCode, Outcome};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::routes::{Job, WorkClass, refusal};

/// One lane's capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneLimits {
    /// Requests running at once.
    pub workers: usize,
    /// Requests admitted and waiting for a worker; one more is answered `busy`.
    pub waiting: usize,
}

#[derive(Debug)]
pub(crate) struct Lanes {
    control: Lane,
    stop: Lane,
    bulk: Lane,
}

#[derive(Debug)]
struct Lane {
    /// How a `busy` answer names the lane.
    name: &'static str,
    limits: LaneLimits,
    workers: Arc<Semaphore>,
    /// Places for admitted requests, running or waiting: `workers + waiting` permits.
    admitted: Arc<Semaphore>,
}

impl Lanes {
    pub(crate) fn new(control: LaneLimits, stop: LaneLimits, bulk: LaneLimits) -> Lanes {
        Lanes {
            control: Lane::new("control", control),
            stop: Lane::new("stop", stop),
            bulk: Lane::new("bulk", bulk),
        }
    }

    /// Admits `job` to its lane, or returns the `busy` answer when the lane is full. The
    /// returned future waits for a worker, runs the job in its own task so a panic stays
    /// inside this request, and releases the lane's places only once the job has stopped.
    pub(crate) fn admit(
        &self,
        class: WorkClass,
        job: Job,
    ) -> Result<impl Future<Output = Outcome> + Send + 'static, Outcome> {
        let lane = match class {
            WorkClass::Control => &self.control,
            WorkClass::Stop => &self.stop,
            WorkClass::Bulk => &self.bulk,
        };
        let admitted = Arc::clone(&lane.admitted)
            .try_acquire_owned()
            .map_err(|_| lane.busy())?;
        let workers = Arc::clone(&lane.workers);
        Ok(async move {
            let worker = workers
                .acquire_owned()
                .await
                .expect("lane semaphores are never closed");
            run_contained(job, worker, admitted).await
        })
    }
}

/// Runs `job` in a task of its own and holds both permits until it returns or panics. The
/// task is never aborted, so the permits cannot be released while the operation still runs.
async fn run_contained(
    job: Job,
    _worker: OwnedSemaphorePermit,
    _admitted: OwnedSemaphorePermit,
) -> Outcome {
    match tokio::spawn(job).await {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::error!(
                panicked = error.is_panic(),
                "a request failed; the daemon carries on"
            );
            refusal(
                ErrorCode::Internal,
                "the daemon failed while handling this request",
            )
        }
    }
}

impl Lane {
    /// `limits` passed `Limits::validate`, so the lane has a worker.
    fn new(name: &'static str, limits: LaneLimits) -> Lane {
        Lane {
            name,
            limits,
            workers: Arc::new(Semaphore::new(limits.workers)),
            admitted: Arc::new(Semaphore::new(limits.workers + limits.waiting)),
        }
    }

    fn busy(&self) -> Outcome {
        refusal(
            ErrorCode::Busy,
            &format!(
                "the {} lane is full ({} running, {} waiting); retry later",
                self.name, self.limits.workers, self.limits.waiting
            ),
        )
    }
}
