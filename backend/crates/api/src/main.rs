use std::process::ExitCode;

use infra::config::{ApiConfig, Env};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let config = match ApiConfig::from_env(&Env::from_process()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("api: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };

    let listener = match tokio::net::TcpListener::bind(config.bind_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("api: cannot listen on {}: {err}", config.bind_addr);
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(addr = %config.bind_addr, "api listening");

    let app = api::app(&config);
    match api::serve(
        listener,
        app,
        infra::shutdown::signal(),
        config.common.shutdown_grace,
    )
    .await
    {
        Ok(()) => {
            tracing::info!("api stopped");
            ExitCode::SUCCESS
        }
        Err(err) => {
            tracing::error!(error = %err, "api stopped with an error");
            ExitCode::FAILURE
        }
    }
}
