//! Long-lived workers: a crash is logged and the worker restarted, never silent (report 03).

use std::future::Future;
use std::time::Duration;

const FIRST_RESTART_DELAY: Duration = Duration::from_millis(50);
const LONGEST_RESTART_DELAY: Duration = Duration::from_secs(5);

/// Runs the worker `start` builds, in a task of its own, and starts a new one each time it
/// panics, waiting longer after each crash so a worker that always fails cannot spin. A
/// worker that returns has finished its job, and supervision ends with it.
pub(crate) async fn supervise<F, W>(worker: &'static str, mut start: F)
where
    F: FnMut() -> W,
    W: Future<Output = ()> + Send + 'static,
{
    let mut delay = FIRST_RESTART_DELAY;
    let mut restarts: u64 = 0;
    loop {
        match tokio::spawn(start()).await {
            Ok(()) => return,
            Err(error) if error.is_panic() => {
                restarts += 1;
                tracing::error!(worker, restarts, "worker panicked; restarting it");
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use super::supervise;

    #[tokio::test]
    async fn a_panicking_worker_is_restarted_until_it_finishes() {
        let starts = Arc::new(AtomicU32::new(0));
        let counted = Arc::clone(&starts);
        supervise("test", move || {
            let attempt = counted.fetch_add(1, Ordering::SeqCst) + 1;
            async move {
                assert!(attempt >= 3, "crash number {attempt}");
            }
        })
        .await;
        assert_eq!(starts.load(Ordering::SeqCst), 3);
    }
}
