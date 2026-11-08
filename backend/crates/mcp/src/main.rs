//! MCP server. It holds no business logic: every tool calls the public API
//! on behalf of the user (L16). For now it only starts, listens and stops.

use std::process::ExitCode;

use axum::Router;
use infra::config::{Env, McpConfig};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = match McpConfig::from_env(&Env::from_process()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("mcp: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let listener = match tokio::net::TcpListener::bind(config.bind_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("mcp: cannot listen on {}: {err}", config.bind_addr);
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(addr = %config.bind_addr, api = %config.api_base_url, "mcp listening");

    let server = axum::serve(listener, Router::new()).with_graceful_shutdown(infra::shutdown::signal());
    match server.await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(error = %err, "mcp stopped with an error");
            ExitCode::FAILURE
        }
    }
}
