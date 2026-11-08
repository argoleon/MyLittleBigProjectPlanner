//! REST API service.
//!
//! [`app`] builds the full router; [`serve`] runs it with graceful shutdown.
//! Every route goes through the shared request layer in [`middleware`], so
//! no feature can forget request IDs, timeouts or body limits.

pub mod middleware;
mod routes;

use std::future::{Future, IntoFuture};
use std::time::Duration;

use axum::Router;
use infra::config::ApiConfig;
use tokio::net::TcpListener;

pub use middleware::{HttpSettings, with_middleware};

/// The complete API router, with the shared request layer applied.
pub fn app(config: &ApiConfig) -> Router {
    with_middleware(routes::router(), &HttpSettings::from(config))
}

/// Serves `app` on `listener` until `shutdown` resolves, then lets in-flight
/// requests finish for at most `grace` before returning.
pub async fn serve(
    listener: TcpListener,
    app: Router,
    shutdown: impl Future<Output = ()> + Send + 'static,
    grace: Duration,
) -> std::io::Result<()> {
    let (signalled_tx, signalled_rx) = tokio::sync::oneshot::channel::<()>();
    let server = axum::serve(listener, app).with_graceful_shutdown(async move {
        shutdown.await;
        // The receiver is only gone if `serve` itself returned; nothing to do then.
        let _ = signalled_tx.send(());
    });
    let mut server = tokio::spawn(server.into_future());

    tokio::select! {
        res = &mut server => return flatten(res),
        _ = signalled_rx => {}
    }

    if let Ok(res) = tokio::time::timeout(grace, &mut server).await {
        flatten(res)
    } else {
        tracing::warn!(
            grace_secs = grace.as_secs(),
            "grace period elapsed, aborting in-flight requests"
        );
        server.abort();
        Ok(())
    }
}

fn flatten(res: Result<std::io::Result<()>, tokio::task::JoinError>) -> std::io::Result<()> {
    res.map_err(std::io::Error::other)?
}
