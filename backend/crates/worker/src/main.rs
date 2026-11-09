use std::process::ExitCode;

use infra::config::{Env, WorkerConfig};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "\
Usage: worker [COMMAND]

Commands:
  (none)       Run the background job service
  healthcheck  Exit 0 if the worker listening on WORKER_HEALTH_ADDR is alive, 1 otherwise

Configuration is read from environment variables; see deploy/.env.example.";

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] => run().await,
        ["healthcheck"] => healthcheck().await,
        ["help" | "--help" | "-h"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("worker: unexpected arguments: {}\n\n{USAGE}", args.join(" "));
            ExitCode::from(2)
        }
    }
}

/// Health check of the container image, which has no shell nor curl.
async fn healthcheck() -> ExitCode {
    match infra::config::worker_health_addr(&Env::from_process()) {
        Ok(bind) => infra::health::healthcheck("worker", bind).await,
        Err(err) => {
            eprintln!("worker: configuration error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> ExitCode {
    let config = match WorkerConfig::from_env(&Env::from_process()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("worker: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let listener = match tokio::net::TcpListener::bind(config.health_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("worker: cannot listen on {}: {err}", config.health_addr);
            return ExitCode::FAILURE;
        }
    };

    let token = CancellationToken::new();
    let health = tokio::spawn(worker::serve_health(listener, token.clone()));
    let mut worker = tokio::spawn(worker::run_loop(
        token.clone(),
        config.poll_interval,
        worker::poll_jobs,
    ));
    tracing::info!(health = %config.health_addr, "worker started");

    tokio::select! {
        res = &mut worker => {
            token.cancel();
            return match res {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    tracing::error!(error = %err, "worker loop crashed");
                    ExitCode::FAILURE
                }
            };
        }
        () = infra::shutdown::signal() => token.cancel(),
    }

    let stopped = tokio::time::timeout(config.common.shutdown_grace, worker).await;
    // The health endpoint stops with the token; it holds no work.
    let _ = health.await;
    match stopped {
        Ok(Ok(())) => {
            tracing::info!("worker stopped");
            ExitCode::SUCCESS
        }
        Ok(Err(err)) => {
            tracing::error!(error = %err, "worker loop crashed");
            ExitCode::FAILURE
        }
        Err(_) => {
            tracing::warn!("grace period elapsed, abandoning the in-flight batch");
            ExitCode::FAILURE
        }
    }
}
