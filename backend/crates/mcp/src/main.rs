//! MCP server. It holds no business logic: every tool calls the public API
//! on behalf of the user (L16). For now it only starts, answers its health
//! endpoint and stops.

use std::process::ExitCode;

use axum::Router;
use axum::http::header;
use axum::routing::get;
use infra::config::{Env, McpConfig};
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
Usage: mcp [COMMAND]

Commands:
  (none)       Serve the MCP server
  healthcheck  Exit 0 if the server listening on MCP_BIND_ADDR is alive, 1 otherwise

Configuration is read from environment variables; see deploy/.env.example.";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => serve().await,
        ["healthcheck"] => healthcheck().await,
        ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("mcp: unexpected arguments: {}\n\n{USAGE}", args.join(" "));
            ExitCode::from(2)
        }
    }
}

/// Health check of the container image, which has no shell nor curl.
async fn healthcheck() -> ExitCode {
    match Env::from_process().required::<std::net::SocketAddr>("MCP_BIND_ADDR") {
        Ok(bind) => infra::health::healthcheck("mcp", bind).await,
        Err(err) => {
            eprintln!("mcp: configuration error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> ExitCode {
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

    let app = Router::new().route(
        infra::health::LIVE_PATH,
        get(|| async {
            (
                [(header::CONTENT_TYPE, "application/json")],
                infra::health::LIVE_BODY,
            )
        }),
    );
    let server = axum::serve(listener, app).with_graceful_shutdown(infra::shutdown::signal());
    match server.await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(error = %err, "mcp stopped with an error");
            ExitCode::FAILURE
        }
    }
}
