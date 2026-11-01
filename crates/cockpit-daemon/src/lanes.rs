//! Bounded admission: each lane has a fixed number of workers and of waiting places. A full
//! lane answers `busy` at once; it never queues without bound (report 03).

use std::sync::Arc;
use std::time::Duration;

use cockpit_protocol::{ErrorCode, Outcome};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::bug_reports::{Fault, Faults};
use crate::routes::{Job, WorkClass, refusal};

/// One lane's capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaneLimits {
    /// Requests running at once.
    pub workers: usize,
    /// Requests admitted and waiting for a worker; one more is answered `busy`.
    pub waiting: usize,
}

/// How long a request may run in each lane before the daemon reports it stalled (#427). A
/// stalled request keeps running: the budget only opens a bug report draft.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StallBudgets {
    pub control: Duration,
    pub stop: Duration,
    pub bulk: Duration,
}

impl Default for StallBudgets {
    /// The clients' own deadlines: past them no client waits for the answer any more.
    fn default() -> Self {
        StallBudgets {
            control: Duration::from_secs(30),
            stop: Duration::from_secs(2 * 60),
            bulk: Duration::from_secs(30 * 60),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Lanes {
    control: Lane,
    stop: Lane,
    bulk: Lane,
    faults: Faults,
}

#[derive(Debug)]
struct Lane {
    /// How a `busy` answer names the lane.
    name: &'static str,
    limits: LaneLimits,
    stall_budget: Duration,
    workers: Arc<Semaphore>,
    /// Places for admitted requests, running or waiting: `workers + waiting` permits.
    admitted: Arc<Semaphore>,
}

impl Lanes {
    pub(crate) fn new(
        control: LaneLimits,
        stop: LaneLimits,
        bulk: LaneLimits,
        budgets: StallBudgets,
        faults: Faults,
    ) -> Lanes {
        Lanes {
            control: Lane::new("control", control, budgets.control),
            stop: Lane::new("stop", stop, budgets.stop),
            bulk: Lane::new("bulk", bulk, budgets.bulk),
            faults,
        }
    }

    /// Admits `job` to its lane, or returns the `busy` answer when the lane is full. The
    /// returned future waits for a worker, runs the job in its own task so a panic stays
    /// inside this request, and releases the lane's places only once the job has stopped.
    /// `command` names the request's kind in the faults it reports: a panic, a run past the
    /// lane's stall budget, an `internal` answer.
    pub(crate) fn admit(
        &self,
        class: WorkClass,
        command: String,
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
        let watch = Watch {
            lane: lane.name,
            budget: lane.stall_budget,
            command,
            faults: self.faults.clone(),
        };
        Ok(async move {
            let worker = workers
                .acquire_owned()
                .await
                .expect("lane semaphores are never closed");
            run_contained(job, watch, worker, admitted).await
        })
    }
}

/// What a running request reports about itself.
struct Watch {
    lane: &'static str,
    budget: Duration,
    command: String,
    faults: Faults,
}

/// Runs `job` in a task of its own and holds both permits until it returns or panics. The
/// task is never aborted, so the permits cannot be released while the operation still runs;
/// past its budget it is reported stalled and still awaited.
async fn run_contained(
    job: Job,
    watch: Watch,
    _worker: OwnedSemaphorePermit,
    _admitted: OwnedSemaphorePermit,
) -> Outcome {
    let mut task = tokio::spawn(job);
    let joined = tokio::select! {
        joined = &mut task => joined,
        () = tokio::time::sleep(watch.budget) => {
            tracing::error!(
                lane = watch.lane,
                command = %watch.command,
                budget_seconds = watch.budget.as_secs(),
                "a request runs past its lane's budget"
            );
            watch.faults.report(Fault::Stall {
                lane: watch.lane,
                command: watch.command.clone(),
                budget: watch.budget,
            });
            (&mut task).await
        }
    };
    match joined {
        Ok(outcome) => {
            if matches!(&outcome, Outcome::Error(error) if error.code == ErrorCode::Internal) {
                watch.faults.report(Fault::Internal {
                    command: watch.command,
                });
            }
            outcome
        }
        Err(error) => {
            tracing::error!(
                panicked = error.is_panic(),
                command = %watch.command,
                "a request failed; the daemon carries on"
            );
            if error.is_panic() {
                watch.faults.report(Fault::RequestPanic {
                    command: watch.command,
                });
            }
            refusal(
                ErrorCode::Internal,
                "the daemon failed while handling this request",
            )
        }
    }
}

impl Lane {
    /// `limits` passed `Limits::validate`, so the lane has a worker.
    fn new(name: &'static str, limits: LaneLimits, stall_budget: Duration) -> Lane {
        Lane {
            name,
            limits,
            stall_budget,
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
