//! Background job service.
//!
//! The job queue (PostgreSQL `SKIP LOCKED`) arrives in L02; for now the main
//! loop polls a no-op batch. The shutdown contract is already final:
//! cancellation is checked between batches only, so a batch that has started
//! always finishes.

use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

/// Result of one poll of the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Batch {
    /// Jobs were processed: poll again immediately.
    Processed,
    /// The queue was empty: wait `poll_interval` before polling again.
    Idle,
}

/// Runs `next_batch` until `token` is cancelled.
pub async fn run_loop<F, Fut>(token: CancellationToken, poll_interval: Duration, mut next_batch: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Batch>,
{
    while !token.is_cancelled() {
        // Not raced against the token: an in-flight batch is never interrupted.
        let batch = next_batch().await;
        if batch == Batch::Idle {
            tokio::select! {
                () = token.cancelled() => {}
                () = tokio::time::sleep(poll_interval) => {}
            }
        }
    }
    tracing::info!("worker loop stopped");
}

/// Placeholder batch until the job queue exists.
pub async fn poll_jobs() -> Batch {
    tokio::task::yield_now().await;
    Batch::Idle
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn in_flight_batch_finishes_after_cancellation() {
        let token = CancellationToken::new();
        let started = Arc::new(AtomicU32::new(0));
        let finished = Arc::new(AtomicU32::new(0));

        let worker = {
            let (token, started, finished) = (token.clone(), started.clone(), finished.clone());
            tokio::spawn(run_loop(token, Duration::from_millis(10), move || {
                let (started, finished) = (started.clone(), finished.clone());
                async move {
                    started.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    finished.fetch_add(1, Ordering::SeqCst);
                    Batch::Processed
                }
            }))
        };

        tokio::time::sleep(Duration::from_millis(50)).await;
        token.cancel();
        worker.await.unwrap();

        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert_eq!(finished.load(Ordering::SeqCst), 1, "the started batch completed");
    }

    #[tokio::test]
    async fn idle_loop_stops_without_waiting_for_the_poll_interval() {
        let token = CancellationToken::new();
        let worker = tokio::spawn(run_loop(token.clone(), Duration::from_secs(60), poll_jobs));
        tokio::time::sleep(Duration::from_millis(20)).await;
        token.cancel();
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .expect("loop stopped promptly")
            .unwrap();
    }

    #[tokio::test]
    async fn processed_batches_are_polled_back_to_back() {
        let token = CancellationToken::new();
        let calls = Arc::new(AtomicU32::new(0));
        let worker = {
            let (token, calls) = (token.clone(), calls.clone());
            tokio::spawn(run_loop(token.clone(), Duration::from_secs(60), move || {
                let (token, calls) = (token.clone(), calls.clone());
                async move {
                    if calls.fetch_add(1, Ordering::SeqCst) == 4 {
                        token.cancel();
                    }
                    Batch::Processed
                }
            }))
        };
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .expect("no poll interval between processed batches")
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 5);
    }
}
