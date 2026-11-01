//! Long-lived workers: a crash is logged and the worker restarted, never silent (report 03).

use std::future::Future;
use std::time::{Duration, Instant};

use tokio::task::JoinHandle;

use crate::bug_reports::{Fault, Faults};

const FIRST_RESTART_DELAY: Duration = Duration::from_millis(50);
const LONGEST_RESTART_DELAY: Duration = Duration::from_secs(5);
/// A worker that ran this long before crashing was healthy: its restart waits the shortest
/// delay again, not the longest one an early crash loop reached.
const HEALTHY_RUN: Duration = Duration::from_secs(60);

/// Runs the worker `start` builds, in a task of its own, and starts a new one each time it
/// panics, waiting longer after each quick crash so a worker that always fails cannot spin.
/// A worker that returns has finished its job, and supervision ends with it. Dropping or
/// aborting the supervision aborts the running worker too. Each panic goes to `faults`.
pub(crate) async fn supervise<F, W>(worker: &'static str, faults: Faults, mut start: F)
where
    F: FnMut() -> W,
    W: Future<Output = ()> + Send + 'static,
{
    let mut delay = FIRST_RESTART_DELAY;
    let mut restarts: u64 = 0;
    loop {
        let started = Instant::now();
        let mut running = AbortOnDrop(tokio::spawn(start()));
        match (&mut running.0).await {
            Ok(()) => return,
            Err(error) if error.is_panic() => {
                restarts += 1;
                if started.elapsed() >= HEALTHY_RUN {
                    delay = FIRST_RESTART_DELAY;
                }
                tracing::error!(worker, restarts, "worker panicked; restarting it");
                faults.report(Fault::WorkerPanic { worker });
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(LONGEST_RESTART_DELAY);
            }
            Err(_) => {
                tracing::error!(worker, "worker was cancelled; supervision ends");
                return;
            }
        }
    }
}

struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::supervise;
    use crate::bug_reports::{Fault, Faults};

    #[tokio::test]
    async fn a_panicking_worker_is_restarted_until_it_finishes() {
        let starts = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&starts);
        let (faults, mut reported) = Faults::channel();
        supervise("test", faults, move || {
            let attempt = counted.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                assert!(attempt >= 3, "crash number {attempt}");
            }
        })
        .await;
        assert_eq!(starts.load(Ordering::SeqCst), 3);
        for _ in 0..2 {
            assert_eq!(
                reported.try_recv().unwrap(),
                Fault::WorkerPanic { worker: "test" }
            );
        }
        assert!(reported.try_recv().is_err());
    }
}
