//! Background job service: runs the jobs of the PostgreSQL queue
//! ([`jobs`]). Cancellation is checked between batches only, so a batch that
//! has started always finishes.

pub mod jobs;

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

/// Serves the health endpoints on `listener` until `token` is cancelled:
/// `/health/live`, and `/health/ready`, which checks `dependencies`. The
/// worker exposes nothing else over HTTP.
pub async fn serve_health(
    listener: tokio::net::TcpListener,
    token: CancellationToken,
    dependencies: std::sync::Arc<[infra::health::Dependency]>,
) {
    use axum::http::{StatusCode, header};
    let app = axum::Router::new()
        .route(
            infra::health::LIVE_PATH,
            axum::routing::get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    infra::health::LIVE_BODY,
                )
            }),
        )
        .route(
            infra::health::READY_PATH,
            axum::routing::get(move || {
                let dependencies = dependencies.clone();
                async move {
                    let report = infra::health::readiness(&dependencies).await;
                    let status =
                        StatusCode::from_u16(report.status_code()).unwrap_or(StatusCode::SERVICE_UNAVAILABLE);
                    (
                        status,
                        [(header::CONTENT_TYPE, "application/json")],
                        report.to_json(),
                    )
                }
            }),
        );
    let server = axum::serve(listener, app).with_graceful_shutdown(async move { token.cancelled().await });
    if let Err(err) = server.await {
        tracing::error!(error = %err, "health endpoint stopped with an error");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn readiness_reports_unreachable_dependencies() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_addr = closed.local_addr().unwrap().to_string();
        drop(closed);
        let token = CancellationToken::new();
        let server = tokio::spawn(serve_health(
            listener,
            token.clone(),
            std::sync::Arc::from([infra::health::Dependency::Tcp {
                name: "valkey",
                address: closed_addr,
            }]),
        ));

        infra::health::probe(addr, infra::health::LIVE_PATH, infra::health::PROBE_TIMEOUT)
            .await
            .unwrap();
        let ready = infra::health::probe(addr, infra::health::READY_PATH, infra::health::PROBE_TIMEOUT).await;
        assert!(
            matches!(&ready, Err(infra::health::ProbeError::Status(line)) if line.contains("503")),
            "{ready:?}"
        );
        token.cancel();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn health_endpoint_answers_until_cancelled() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let token = CancellationToken::new();
        let server = tokio::spawn(serve_health(listener, token.clone(), std::sync::Arc::from([])));

        infra::health::probe(addr, infra::health::LIVE_PATH, infra::health::PROBE_TIMEOUT)
            .await
            .unwrap();
        assert!(
            infra::health::probe(addr, "/other", infra::health::PROBE_TIMEOUT)
                .await
                .is_err(),
            "only the health endpoint exists"
        );

        token.cancel();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("stops when cancelled")
            .unwrap();
    }
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
        let worker = tokio::spawn(run_loop(token.clone(), Duration::from_secs(60), || async {
            Batch::Idle
        }));
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
