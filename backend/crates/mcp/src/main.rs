//! MCP server. It holds no business logic: every tool calls the public API
//! on behalf of the user (L16). For now it only starts, answers its health
//! endpoints and stops.

use std::future::Future;
use std::process::ExitCode;
use std::sync::Arc;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::routing::get;
use infra::config::{Env, McpConfig};
use infra::health::Dependency;

const USAGE: &str = "\
Usage: mcp [COMMAND]

Commands:
  (none)       Serve the MCP server
  healthcheck [--ready]
               Exit 0 if the server listening on MCP_BIND_ADDR is alive (or, with
               --ready, can serve: the API answers)

Configuration is read from environment variables; see deploy/.env.example.";

// Not `#[tokio::main]`: telemetry starts before the runtime and stops after
// it (see infra::telemetry).
fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [] => serve(),
        ["healthcheck", rest @ ..] => match infra::health::healthcheck_path(rest) {
            Some(path) => block_on(healthcheck(path)),
            None => usage_error(&args),
        },
        ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => usage_error(&args),
    }
}

fn usage_error(args: &[&str]) -> ExitCode {
    eprintln!("mcp: unexpected arguments: {}\n\n{USAGE}", args.join(" "));
    ExitCode::from(2)
}

fn block_on(task: impl Future<Output = ExitCode>) -> ExitCode {
    match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime.block_on(task),
        Err(err) => {
            eprintln!("mcp: cannot start the async runtime: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Health check of the container image, which has no shell nor curl.
async fn healthcheck(path: &str) -> ExitCode {
    match Env::from_process().required::<std::net::SocketAddr>("MCP_BIND_ADDR") {
        Ok(bind) => infra::health::healthcheck("mcp", bind, path).await,
        Err(err) => {
            eprintln!("mcp: configuration error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn serve() -> ExitCode {
    let config = match McpConfig::from_env(&Env::from_process()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("mcp: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };
    let telemetry = match infra::telemetry::init("mcp", &config.telemetry) {
        Ok(telemetry) => telemetry,
        Err(err) => {
            eprintln!("mcp: cannot start telemetry: {err}");
            return ExitCode::FAILURE;
        }
    };
    let code = block_on(run(config));
    telemetry.shutdown();
    code
}

async fn run(config: McpConfig) -> ExitCode {
    let listener = match tokio::net::TcpListener::bind(config.bind_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("mcp: cannot listen on {}: {err}", config.bind_addr);
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(addr = %config.bind_addr, api = %config.api_base_url, "mcp listening");

    // The MCP server can serve only if the API answers.
    let dependencies: Arc<[Dependency]> = infra::health::address_of(&config.api_base_url)
        .map(|address| Dependency::Http { name: "api", address })
        .into_iter()
        .collect();

    let app = Router::new()
        .route(
            infra::health::LIVE_PATH,
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    infra::health::LIVE_BODY,
                )
            }),
        )
        .route(
            infra::health::READY_PATH,
            get(move || {
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
    let server = axum::serve(listener, app).with_graceful_shutdown(infra::shutdown::signal());
    match server.await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(error = %err, "mcp stopped with an error");
            ExitCode::FAILURE
        }
    }
}
