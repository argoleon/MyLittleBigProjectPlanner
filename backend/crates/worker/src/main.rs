use std::future::Future;
use std::process::ExitCode;

use infra::config::{Env, WorkerConfig};
use tokio_util::sync::CancellationToken;

const USAGE: &str = "\
Usage: worker [COMMAND]

Commands:
  (none)       Run the background job service
  healthcheck [--ready]
               Exit 0 if the worker listening on WORKER_HEALTH_ADDR is alive (or, with
               --ready, can work: its dependencies are reachable)

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
    eprintln!("worker: unexpected arguments: {}\n\n{USAGE}", args.join(" "));
    ExitCode::from(2)
}

fn block_on(task: impl Future<Output = ExitCode>) -> ExitCode {
    match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime.block_on(task),
        Err(err) => {
            eprintln!("worker: cannot start the async runtime: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Health check of the container image, which has no shell nor curl.
async fn healthcheck(path: &str) -> ExitCode {
    match infra::config::worker_health_addr(&Env::from_process()) {
        Ok(bind) => infra::health::healthcheck("worker", bind, path).await,
        Err(err) => {
            eprintln!("worker: configuration error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn serve() -> ExitCode {
    let config = match WorkerConfig::from_env(&Env::from_process()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("worker: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };
    let telemetry = match infra::telemetry::init("worker", &config.telemetry) {
        Ok(telemetry) => telemetry,
        Err(err) => {
            eprintln!("worker: cannot start telemetry: {err}");
            return ExitCode::FAILURE;
        }
    };
    let code = block_on(run(config));
    telemetry.shutdown();
    code
}

async fn run(config: WorkerConfig) -> ExitCode {
    let pool = match infra::db::connect_lazy(&config.database) {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("worker: invalid DATABASE_URL: {err}");
            return ExitCode::FAILURE;
        }
    };
    infra::telemetry::observe_pool(&pool);
    let mailer = match infra::email::Mailer::new(&config.mail.smtp_url, &config.mail.from) {
        Ok(mailer) => mailer,
        Err(err) => {
            eprintln!("worker: invalid mail settings: {err}");
            return ExitCode::FAILURE;
        }
    };
    let mut dependencies = vec![infra::health::Dependency::Postgres(pool.clone())];
    dependencies.extend(config.shared.dependencies());

    let listener = match tokio::net::TcpListener::bind(config.health_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("worker: cannot listen on {}: {err}", config.health_addr);
            return ExitCode::FAILURE;
        }
    };

    let token = CancellationToken::new();
    let health = tokio::spawn(worker::serve_health(listener, token.clone(), dependencies.into()));
    let runner = std::sync::Arc::new(worker::jobs::JobRunner::new(pool));
    let valkey = match infra::valkey::connect_lazy(&config.valkey) {
        Ok(valkey) => valkey,
        Err(err) => {
            eprintln!("worker: invalid VALKEY_URL: {err}");
            return ExitCode::FAILURE;
        }
    };
    let exports = infra::exports::ExportStore::new(valkey, "mlbpp:");
    let handler = std::sync::Arc::new(worker::jobs::Jobs { mailer, exports });
    tracing::info!(health = %config.health_addr, worker_id = runner.worker_id(), "worker started");
    let mut worker = tokio::spawn(worker::run_loop(token.clone(), config.poll_interval, move || {
        let (runner, handler) = (runner.clone(), handler.clone());
        async move { runner.next_batch(handler.as_ref()).await }
    }));

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
