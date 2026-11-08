use std::process::ExitCode;

use infra::config::{Env, WorkerConfig};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = match WorkerConfig::from_env(&Env::from_process()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("worker: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let token = CancellationToken::new();
    let mut worker = tokio::spawn(worker::run_loop(
        token.clone(),
        config.poll_interval,
        worker::poll_jobs,
    ));
    tracing::info!("worker started");

    tokio::select! {
        res = &mut worker => {
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

    match tokio::time::timeout(config.common.shutdown_grace, worker).await {
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
