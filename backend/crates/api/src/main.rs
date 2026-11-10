use std::future::Future;
use std::process::ExitCode;

use api::cli::{self, Command, MemberArgs};
use infra::bootstrap::BootstrapOutcome;
use infra::config::{
    ApiConfig, BootstrapConfig, DatabaseConfig, Env, MigrateConfig, Secret, TelemetryConfig,
};
use infra::telemetry::Telemetry;

// Not `#[tokio::main]`: telemetry starts before the runtime and stops after
// it (see infra::telemetry).
fn main() -> ExitCode {
    match cli::parse(std::env::args().skip(1)) {
        Ok(Command::Serve) => serve(),
        Ok(Command::Migrate) => run_with_logs(migrate()),
        Ok(Command::OpenApi) => openapi(),
        Ok(Command::Healthcheck { ready }) => runtime_block_on(healthcheck(ready)),
        Ok(Command::AddMember(args)) => runtime_block_on(add_member(args)),
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

fn runtime_block_on(task: impl Future<Output = ExitCode>) -> ExitCode {
    match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime.block_on(task),
        Err(err) => {
            eprintln!("api: cannot start the async runtime: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Runs a one-off command with logs on standard output, no export.
fn run_with_logs(task: impl Future<Output = ExitCode>) -> ExitCode {
    let telemetry = match TelemetryConfig::from_env(&Env::from_process()) {
        Ok(mut config) => {
            config.otlp_endpoint = None;
            infra::telemetry::init("api", &config)
        }
        Err(err) => {
            eprintln!("api: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };
    let code = runtime_block_on(task);
    if let Ok(telemetry) = telemetry {
        telemetry.shutdown();
    }
    code
}

/// Health check of the container image, which has no shell nor curl.
async fn healthcheck(ready: bool) -> ExitCode {
    let path = if ready {
        infra::health::READY_PATH
    } else {
        infra::health::LIVE_PATH
    };
    match Env::from_process().required::<std::net::SocketAddr>("API_BIND_ADDR") {
        Ok(bind) => infra::health::healthcheck("api", bind, path).await,
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

async fn add_member(args: MemberArgs) -> ExitCode {
    let database = match DatabaseConfig::from_env(&Env::from_process()) {
        Ok(database) => database,
        Err(err) => {
            eprintln!("api: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };
    let mut password = String::new();
    if let Err(err) = std::io::stdin().read_line(&mut password) {
        eprintln!("api: cannot read the password from standard input: {err}");
        return ExitCode::FAILURE;
    }
    let password = password.trim_end_matches(['\r', '\n']);
    let pool = match infra::db::connect_lazy(&database) {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("api: invalid DATABASE_URL: {err}");
            return ExitCode::FAILURE;
        }
    };
    match cli::add_member(&pool, &args, password).await {
        Ok(added) if added.new_account => {
            println!("added {} to {} ({})", args.email, args.tenant, added.account_id);
            ExitCode::SUCCESS
        }
        Ok(added) => {
            println!(
                "linked the existing account {} to {}; its password is unchanged",
                added.account_id, args.tenant
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("api: cannot add the member: {err}");
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
    if let Err(err) = infra::db::migrate(&config.migration_url).await {
        eprintln!("api: migration failed: {err}");
        return ExitCode::FAILURE;
    }
    tracing::info!("migrations applied");
    if bootstrap(&config.migration_url, config.bootstrap.as_ref()).await {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Creates the first tenant and super admin when configured and not done
/// yet. False when it failed (reported on standard error).
async fn bootstrap(migration_url: &Secret, config: Option<&BootstrapConfig>) -> bool {
    let Some(config) = config else {
        return true;
    };
    match infra::bootstrap::run(migration_url, config).await {
        Ok(BootstrapOutcome::AlreadyDone) => {
            tracing::info!("bootstrap skipped: a platform super admin exists");
            true
        }
        Ok(BootstrapOutcome::Done {
            tenant_created,
            account_created,
            ..
        }) => {
            tracing::info!(
                tenant = %config.tenant_slug,
                tenant_created,
                account_created,
                "bootstrapped the first tenant and its platform super admin"
            );
            true
        }
        Err(err) => {
            eprintln!("api: bootstrap failed: {err}");
            false
        }
    }
}

fn serve() -> ExitCode {
    let config = match ApiConfig::from_env(&Env::from_process()) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("api: configuration error: {err}");
            return ExitCode::FAILURE;
        }
    };
    let telemetry: Telemetry = match infra::telemetry::init("api", &config.telemetry) {
        Ok(telemetry) => telemetry,
        Err(err) => {
            eprintln!("api: cannot start telemetry: {err}");
            return ExitCode::FAILURE;
        }
    };
    let code = runtime_block_on(run(config));
    telemetry.shutdown();
    code
}

async fn run(config: ApiConfig) -> ExitCode {
    if let Some(migration_url) = &config.migration_url {
        if let Err(err) = infra::db::migrate(migration_url).await {
            eprintln!("api: migration failed: {err}");
            return ExitCode::FAILURE;
        }
        tracing::info!("migrations applied");
        if !bootstrap(migration_url, config.bootstrap.as_ref()).await {
            return ExitCode::FAILURE;
        }
    }

    let pool = match infra::db::connect_lazy(&config.database) {
        Ok(pool) => pool,
        Err(err) => {
            eprintln!("api: invalid DATABASE_URL: {err}");
            return ExitCode::FAILURE;
        }
    };
    infra::telemetry::observe_pool(&pool);

    let listener = match tokio::net::TcpListener::bind(config.bind_addr).await {
        Ok(listener) => listener,
        Err(err) => {
            eprintln!("api: cannot listen on {}: {err}", config.bind_addr);
            return ExitCode::FAILURE;
        }
    };
    tracing::info!(addr = %config.bind_addr, "api listening");

    let valkey = match infra::valkey::connect_lazy(&config.valkey) {
        Ok(valkey) => valkey,
        Err(err) => {
            eprintln!("api: invalid VALKEY_URL: {err}");
            return ExitCode::FAILURE;
        }
    };
    let mfa = match infra::mfa::MfaCipher::new(&config.mfa_key) {
        Ok(mfa) => mfa,
        Err(err) => {
            eprintln!("api: {err}");
            return ExitCode::FAILURE;
        }
    };
    let auth = api::Auth::new(valkey, "mlbpp:", config.public_url.clone(), config.sessions, mfa);
    let state = api::AppState::new(pool, config.tenancy.clone(), auth)
        .with_dependencies(config.shared.dependencies());
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
