//! REST API service.
//!
//! [`app`] builds the full router; [`serve`] runs it with graceful shutdown.
//! Every route goes through the shared request layer in [`middleware`], so
//! no feature can forget request IDs, timeouts or body limits.

pub mod auth;
pub mod cli;
pub mod correlation;
pub mod etag;
pub mod extract;
pub mod fields;
pub mod groups;
pub mod idempotency;
pub mod invitations;
pub mod me;
pub mod members;
pub mod mfa;
pub mod middleware;
pub mod openapi;
pub mod pagination;
pub mod platform;
pub mod problem;
pub mod reference_items;
mod routes;
pub mod sso;
pub mod tenant;

use std::future::{Future, IntoFuture};
use std::time::Duration;

use axum::Router;
use tokio::net::TcpListener;

pub use auth::Auth;
pub use middleware::{HttpSettings, with_middleware};
pub use tenant::{AppState, PublicTenantTx, TenantTx};

/// The complete API router, with the shared request layer applied.
pub fn app(state: AppState, settings: &HttpSettings) -> Router {
    with_middleware(routes::router(state), settings)
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
    // The peer address is the client's IP when no proxy forwards it (rate limits).
    let app = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
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
