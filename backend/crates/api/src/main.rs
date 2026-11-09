use std::process::ExitCode;

use api::cli::{self, Command};
use infra::config::{ApiConfig, Env, MigrateConfig};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    match cli::parse(std::env::args().skip(1)) {
        Ok(Command::Serve) => serve().await,
        Ok(Command::Migrate) => migrate().await,
        Ok(Command::OpenApi) => openapi(),
        Ok(Command::Healthcheck) => healthcheck().await,
        Ok(Command::Help) => {
            println!("{}", cli::USAGE);
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("api: {err}\n\n{}", cli::USAGE);
            ExitCode::from(2)
        }
    }
}

/// Health check of the container image, which has no shell nor curl.
async fn healthcheck() -> ExitCode {
    match Env::from_process().required::<std::net::SocketAddr>("API_BIND_ADDR") {
        Ok(bind) => infra::health::healthcheck("api", bind).await,
        Err(err) => {
            eprintln!("api: configuration error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Needs no configuration nor database: the contract comes from the code.
fn openapi() -> ExitCode {
    match api::openapi::contract_json() {
        Ok(json) => {
            print!("{json}");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("api: cannot serialize the OpenAPI contract: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn migrate() -> ExitCode {
    let config = match MigrateConfig::from_env(&Env::from_process()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("api: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };
    match infra::db::migrate(&config.migration_url).await {
        Ok(()) => {
            tracing::info!("migrations applied");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("api: migration failed: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> ExitCode {
    let config = match ApiConfig::from_env(&Env::from_process()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("api: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let Some(migration_url) = &config.migration_url {
        if let Err(err) = infra::db::migrate(migration_url).await {
            eprintln!("api: migration failed: {err}");
            return ExitCode::FAILURE;
        }
        tracing::info!("migrations applied");
    }

    let pool = match infra::db::connect_lazy(&config.database) {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("api: invalid DATABASE_URL: {err}");
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

    let state = api::AppState::new(pool, config.tenancy.clone());
    let app = api::app(state, &api::HttpSettings::from(&config));
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
