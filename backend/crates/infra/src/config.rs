//! Typed configuration read from environment variables.
//!
//! Every variable can also be provided through a mounted file: set
//! `NAME_FILE=/run/secrets/name` instead of `NAME`. This is how secrets are
//! passed in Docker and Kubernetes. Setting both is an error.
//!
//! Secrets never have a default value. Every variable is documented in
//! `deploy/.env.example`.

use std::collections::HashMap;
use std::fmt::Display;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use domain::tenancy::Tenancy;

/// A configuration problem. The message always names the variable.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required environment variable {name}")]
    Missing { name: String },
    #[error("invalid value for environment variable {name}: {reason}")]
    Invalid { name: String, reason: String },
    #[error("both {name} and {name}_FILE are set; set only one of them")]
    Conflict { name: String },
    #[error("cannot read {name}_FILE at {}: {source}", path.display())]
    File {
        name: String,
        path: PathBuf,
        source: std::io::Error,
    },
}

/// A snapshot of the variables to read from.
///
/// Built from the process environment in production, and from a plain map in
/// tests, so tests never touch the global environment.
#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: HashMap<String, String>,
}

impl Env {
    /// Snapshot of the current process environment.
    pub fn from_process() -> Self {
        Self {
            vars: std::env::vars().collect(),
        }
    }

    /// Environment made of the given pairs (for tests and tools).
    pub fn from_pairs<K: Into<String>, V: Into<String>>(pairs: impl IntoIterator<Item = (K, V)>) -> Self {
        Self {
            vars: pairs.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
        }
    }

    /// Raw value of `name`, read directly or from the file named by `name_FILE`.
    /// Empty values count as missing.
    pub fn get(&self, name: &str) -> Result<Option<String>, ConfigError> {
        let direct = self.vars.get(name).filter(|v| !v.is_empty());
        let file = self.vars.get(&format!("{name}_FILE")).filter(|v| !v.is_empty());
        match (direct, file) {
            (Some(_), Some(_)) => Err(ConfigError::Conflict {
                name: name.to_owned(),
            }),
            (Some(value), None) => Ok(Some(value.clone())),
            (None, Some(path)) => {
                let path = PathBuf::from(path);
                let content = std::fs::read_to_string(&path).map_err(|source| ConfigError::File {
                    name: name.to_owned(),
                    path,
                    source,
                })?;
                let value = content.trim_end_matches(['\n', '\r']).to_owned();
                Ok(Some(value).filter(|v| !v.is_empty()))
            }
            (None, None) => Ok(None),
        }
    }

    /// Required value, parsed.
    pub fn required<T>(&self, name: &str) -> Result<T, ConfigError>
    where
        T: FromStr,
        T::Err: Display,
    {
        let raw = self.get(name)?.ok_or_else(|| ConfigError::Missing {
            name: name.to_owned(),
        })?;
        parse(name, &raw)
    }

    /// Optional value, parsed, with a default when absent.
    pub fn optional<T>(&self, name: &str, default: T) -> Result<T, ConfigError>
    where
        T: FromStr,
        T::Err: Display,
    {
        match self.get(name)? {
            Some(raw) => parse(name, &raw),
            None => Ok(default),
        }
    }

    /// Optional duration expressed in whole seconds; zero is rejected.
    pub fn seconds(&self, name: &str, default_secs: u64) -> Result<Duration, ConfigError> {
        let secs: u64 = self.optional(name, default_secs)?;
        if secs == 0 {
            return Err(invalid(name, "must be greater than 0"));
        }
        Ok(Duration::from_secs(secs))
    }

    /// Optional duration expressed in milliseconds; zero is rejected.
    pub fn millis(&self, name: &str, default_ms: u64) -> Result<Duration, ConfigError> {
        let ms: u64 = self.optional(name, default_ms)?;
        if ms == 0 {
            return Err(invalid(name, "must be greater than 0"));
        }
        Ok(Duration::from_millis(ms))
    }
}

fn parse<T>(name: &str, raw: &str) -> Result<T, ConfigError>
where
    T: FromStr,
    T::Err: Display,
{
    raw.trim()
        .parse()
        .map_err(|e: T::Err| invalid(name, &e.to_string()))
}

fn invalid(name: &str, reason: &str) -> ConfigError {
    ConfigError::Invalid {
        name: name.to_owned(),
        reason: reason.to_owned(),
    }
}

/// Settings shared by every service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommonConfig {
    /// Maximum time given to in-flight requests or jobs after SIGTERM.
    pub shutdown_grace: Duration,
}

impl CommonConfig {
    pub fn from_env(env: &Env) -> Result<Self, ConfigError> {
        Ok(Self {
            shutdown_grace: env.seconds("SHUTDOWN_GRACE_SECS", 30)?,
        })
    }
}

/// A sensitive value (password, connection URL with credentials).
/// Its `Debug` output is redacted, so it never ends up in logs.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The actual value; pass it straight to the library that needs it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(***)")
    }
}

impl FromStr for Secret {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

/// PostgreSQL connection settings of a service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseConfig {
    /// Connection URL of the runtime role (`mlbpp_app`).
    pub url: Secret,
    pub max_connections: u32,
    pub acquire_timeout: Duration,
}

impl DatabaseConfig {
    pub fn from_env(env: &Env) -> Result<Self, ConfigError> {
        let max_connections: u32 = env.optional("DATABASE_MAX_CONNECTIONS", 10)?;
        if max_connections == 0 {
            return Err(invalid("DATABASE_MAX_CONNECTIONS", "must be greater than 0"));
        }
        Ok(Self {
            url: env.required("DATABASE_URL")?,
            max_connections,
            acquire_timeout: env.seconds("DATABASE_ACQUIRE_TIMEOUT_SECS", 5)?,
        })
    }
}

/// Single- or multi-tenant mode (specification §3).
pub fn tenancy_from_env(env: &Env) -> Result<Tenancy, ConfigError> {
    let mode: String = env.optional("TENANCY_MODE", "single".to_owned())?;
    match mode.as_str() {
        "single" => {
            let default_slug: String = env.optional("TENANT_DEFAULT_SLUG", "default".to_owned())?;
            if !domain::tenancy::is_valid_slug(&default_slug) {
                return Err(invalid(
                    "TENANT_DEFAULT_SLUG",
                    "must be 1-63 characters among a-z, 0-9 and -, not starting or ending with -",
                ));
            }
            Ok(Tenancy::Single { default_slug })
        }
        "multi" => {
            let base_domain: String = env.required("TENANT_BASE_DOMAIN")?;
            let base_domain = base_domain.trim_end_matches('.').to_ascii_lowercase();
            if base_domain.is_empty() || base_domain.contains([':', '/', ' ']) {
                return Err(invalid(
                    "TENANT_BASE_DOMAIN",
                    "must be a bare domain such as example.org",
                ));
            }
            Ok(Tenancy::Multi { base_domain })
        }
        _ => Err(invalid("TENANCY_MODE", "must be single or multi")),
    }
}

/// Shared services a service depends on besides PostgreSQL. Each one is
/// optional until the code using it arrives (Valkey: L02, S3: L05); when
/// configured, `/health/ready` checks that it is reachable.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SharedServices {
    /// `host:port` of Valkey, from `VALKEY_URL` (secret: it holds the password).
    pub valkey: Option<String>,
    /// `host:port` of the S3 storage, from `S3_ENDPOINT`.
    pub s3: Option<String>,
}

impl SharedServices {
    pub fn from_env(env: &Env) -> Result<Self, ConfigError> {
        let address = |name: &str| -> Result<Option<String>, ConfigError> {
            match env.get(name)? {
                None => Ok(None),
                Some(url) => crate::health::address_of(&url)
                    .map(Some)
                    .ok_or_else(|| invalid(name, "must be a URL such as scheme://host:port")),
            }
        };
        Ok(Self {
            valkey: address("VALKEY_URL")?,
            s3: address("S3_ENDPOINT")?,
        })
    }

    /// The readiness checks of these services.
    pub fn dependencies(&self) -> Vec<crate::health::Dependency> {
        let mut dependencies = Vec::new();
        if let Some(address) = &self.valkey {
            dependencies.push(crate::health::Dependency::Tcp {
                name: "valkey",
                address: address.clone(),
            });
        }
        if let Some(address) = &self.s3 {
            dependencies.push(crate::health::Dependency::Tcp {
                name: "s3",
                address: address.clone(),
            });
        }
        dependencies
    }
}

/// Configuration of the `api` service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiConfig {
    pub common: CommonConfig,
    pub bind_addr: SocketAddr,
    pub request_timeout: Duration,
    pub body_limit_bytes: usize,
    pub database: DatabaseConfig,
    /// Owner-role URL; when set, the api applies pending migrations at startup.
    pub migration_url: Option<Secret>,
    pub tenancy: Tenancy,
    pub shared: SharedServices,
    pub telemetry: TelemetryConfig,
}

impl ApiConfig {
    pub fn from_env(env: &Env) -> Result<Self, ConfigError> {
        let body_limit_bytes: usize = env.optional("API_BODY_LIMIT_BYTES", 2 * 1024 * 1024)?;
        if body_limit_bytes == 0 {
            return Err(invalid("API_BODY_LIMIT_BYTES", "must be greater than 0"));
        }
        Ok(Self {
            common: CommonConfig::from_env(env)?,
            bind_addr: env.required("API_BIND_ADDR")?,
            request_timeout: env.seconds("API_REQUEST_TIMEOUT_SECS", 30)?,
            body_limit_bytes,
            database: DatabaseConfig::from_env(env)?,
            migration_url: env.get("DATABASE_MIGRATION_URL")?.map(Secret),
            tenancy: tenancy_from_env(env)?,
            shared: SharedServices::from_env(env)?,
            telemetry: TelemetryConfig::from_env(env)?,
        })
    }
}

/// Configuration of the `api migrate` command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrateConfig {
    /// Owner-role URL (`mlbpp_owner`).
    pub migration_url: Secret,
}

impl MigrateConfig {
    pub fn from_env(env: &Env) -> Result<Self, ConfigError> {
        Ok(Self {
            migration_url: env.required("DATABASE_MIGRATION_URL")?,
        })
    }
}

/// Configuration of the `worker` service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConfig {
    pub common: CommonConfig,
    /// Pause between two polls of the job queue when it is empty.
    pub poll_interval: Duration,
    /// Address of the health endpoint (the worker serves nothing else).
    pub health_addr: SocketAddr,
    /// Optional until the job queue arrives (L02): when set, readiness
    /// checks PostgreSQL.
    pub database: Option<DatabaseConfig>,
    pub shared: SharedServices,
    pub telemetry: TelemetryConfig,
}

/// Default address of the worker's health endpoint.
pub const WORKER_HEALTH_ADDR: &str = "0.0.0.0:8081";

impl WorkerConfig {
    pub fn from_env(env: &Env) -> Result<Self, ConfigError> {
        let database = match env.get("DATABASE_URL")? {
            Some(_) => Some(DatabaseConfig::from_env(env)?),
            None => None,
        };
        Ok(Self {
            common: CommonConfig::from_env(env)?,
            poll_interval: env.millis("WORKER_POLL_INTERVAL_MS", 1000)?,
            health_addr: worker_health_addr(env)?,
            database,
            shared: SharedServices::from_env(env)?,
            telemetry: TelemetryConfig::from_env(env)?,
        })
    }
}

/// `WORKER_HEALTH_ADDR`, also read alone by `worker healthcheck`.
pub fn worker_health_addr(env: &Env) -> Result<SocketAddr, ConfigError> {
    let raw = env.get("WORKER_HEALTH_ADDR")?;
    parse("WORKER_HEALTH_ADDR", raw.as_deref().unwrap_or(WORKER_HEALTH_ADDR))
}

/// Configuration of the `mcp` service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfig {
    pub common: CommonConfig,
    pub bind_addr: SocketAddr,
    /// Base URL of the public API the MCP server calls, e.g. `http://api:8080`.
    pub api_base_url: String,
    pub telemetry: TelemetryConfig,
}

impl McpConfig {
    pub fn from_env(env: &Env) -> Result<Self, ConfigError> {
        let api_base_url: String = env.required("MCP_API_BASE_URL")?;
        if !(api_base_url.starts_with("http://") || api_base_url.starts_with("https://")) {
            return Err(invalid("MCP_API_BASE_URL", "must start with http:// or https://"));
        }
        Ok(Self {
            common: CommonConfig::from_env(env)?,
            bind_addr: env.required("MCP_BIND_ADDR")?,
            api_base_url: api_base_url.trim_end_matches('/').to_owned(),
            telemetry: TelemetryConfig::from_env(env)?,
        })
    }
}

/// Format of the log lines on standard output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// One JSON object per line (production, log collectors).
    Json,
    /// Human-readable lines (development).
    Text,
}

/// Logs, traces and metrics of a service (see `crate::telemetry`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelemetryConfig {
    /// `LOG_FORMAT`: `json` (default) or `text`.
    pub log_format: LogFormat,
    /// `OTEL_EXPORTER_OTLP_ENDPOINT`, e.g. `http://alloy:4318`: base URL of
    /// an OpenTelemetry collector (OTLP over HTTP). Traces and metrics are
    /// exported only when it is set.
    pub otlp_endpoint: Option<String>,
}

impl TelemetryConfig {
    pub fn from_env(env: &Env) -> Result<Self, ConfigError> {
        let log_format = match env.optional("LOG_FORMAT", "json".to_owned())?.as_str() {
            "json" => LogFormat::Json,
            "text" => LogFormat::Text,
            _ => return Err(invalid("LOG_FORMAT", "must be json or text")),
        };
        let otlp_endpoint = match env.get("OTEL_EXPORTER_OTLP_ENDPOINT")? {
            None => None,
            // Exports go to a collector of the same network (Alloy), in plain
            // HTTP; a remote backend is reached through that collector.
            Some(url) if url.starts_with("http://") => Some(url.trim_end_matches('/').to_owned()),
            Some(_) => {
                return Err(invalid(
                    "OTEL_EXPORTER_OTLP_ENDPOINT",
                    "must be an http:// URL of a collector on the same network, such as http://alloy:4318",
                ));
            }
        };
        Ok(Self {
            log_format,
            otlp_endpoint,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn missing_required_variable_names_it() {
        let err = ApiConfig::from_env(&Env::default()).unwrap_err();
        assert!(matches!(&err, ConfigError::Missing { name } if name == "API_BIND_ADDR"));
        assert_eq!(
            err.to_string(),
            "missing required environment variable API_BIND_ADDR"
        );
    }

    #[test]
    fn empty_value_counts_as_missing() {
        let env = Env::from_pairs([("API_BIND_ADDR", "")]);
        assert!(matches!(
            ApiConfig::from_env(&env),
            Err(ConfigError::Missing { .. })
        ));
    }

    #[test]
    fn malformed_value_is_rejected_with_its_name() {
        let env = Env::from_pairs([("API_BIND_ADDR", "not-an-address")]);
        let err = ApiConfig::from_env(&env).unwrap_err();
        assert!(matches!(&err, ConfigError::Invalid { name, .. } if name == "API_BIND_ADDR"));
        assert!(err.to_string().contains("API_BIND_ADDR"));
    }

    // Fake credentials, test only.
    const DB_URL: &str = "postgres://mlbpp_app:secret@db/mlbpp"; // gitleaks:allow
    const OWNER_URL: &str = "postgres://mlbpp_owner:hunter2@db/mlbpp"; // gitleaks:allow

    #[test]
    fn defaults_apply_when_optional_values_are_absent() {
        let env = Env::from_pairs([("API_BIND_ADDR", "0.0.0.0:8080"), ("DATABASE_URL", DB_URL)]);
        let cfg = ApiConfig::from_env(&env).unwrap();
        assert_eq!(cfg.bind_addr, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(cfg.request_timeout, Duration::from_secs(30));
        assert_eq!(cfg.body_limit_bytes, 2 * 1024 * 1024);
        assert_eq!(cfg.common.shutdown_grace, Duration::from_secs(30));
        assert_eq!(cfg.database.url.expose(), DB_URL);
        assert_eq!(cfg.database.max_connections, 10);
        assert_eq!(cfg.database.acquire_timeout, Duration::from_secs(5));
        assert_eq!(cfg.migration_url, None);
        assert_eq!(
            cfg.tenancy,
            Tenancy::Single {
                default_slug: "default".to_owned()
            }
        );
    }

    #[test]
    fn api_requires_a_database_url() {
        let env = Env::from_pairs([("API_BIND_ADDR", "0.0.0.0:8080")]);
        let err = ApiConfig::from_env(&env).unwrap_err();
        assert!(matches!(&err, ConfigError::Missing { name } if name == "DATABASE_URL"));
    }

    #[test]
    fn secrets_are_redacted_in_debug_output() {
        let env = Env::from_pairs([
            ("API_BIND_ADDR", "0.0.0.0:8080"),
            ("DATABASE_URL", DB_URL),
            ("DATABASE_MIGRATION_URL", OWNER_URL),
        ]);
        let debug = format!("{:?}", ApiConfig::from_env(&env).unwrap());
        assert!(!debug.contains("secret") && !debug.contains("hunter2"), "{debug}");
        assert!(debug.contains("Secret(***)"));
    }

    #[test]
    fn multi_tenancy_requires_a_base_domain() {
        let env = Env::from_pairs([("TENANCY_MODE", "multi")]);
        let err = tenancy_from_env(&env).unwrap_err();
        assert!(matches!(&err, ConfigError::Missing { name } if name == "TENANT_BASE_DOMAIN"));

        let env = Env::from_pairs([("TENANCY_MODE", "multi"), ("TENANT_BASE_DOMAIN", "Example.ORG.")]);
        assert_eq!(
            tenancy_from_env(&env).unwrap(),
            Tenancy::Multi {
                base_domain: "example.org".to_owned()
            }
        );

        let env = Env::from_pairs([
            ("TENANCY_MODE", "multi"),
            ("TENANT_BASE_DOMAIN", "https://example.org"),
        ]);
        assert!(matches!(tenancy_from_env(&env), Err(ConfigError::Invalid { .. })));
    }

    #[test]
    fn tenancy_mode_and_default_slug_are_validated() {
        let env = Env::from_pairs([("TENANCY_MODE", "both")]);
        let err = tenancy_from_env(&env).unwrap_err();
        assert!(matches!(&err, ConfigError::Invalid { name, .. } if name == "TENANCY_MODE"));

        let env = Env::from_pairs([("TENANT_DEFAULT_SLUG", "Not Valid")]);
        let err = tenancy_from_env(&env).unwrap_err();
        assert!(matches!(&err, ConfigError::Invalid { name, .. } if name == "TENANT_DEFAULT_SLUG"));
    }

    #[test]
    fn migrate_command_requires_the_owner_url() {
        let err = MigrateConfig::from_env(&Env::default()).unwrap_err();
        assert!(matches!(&err, ConfigError::Missing { name } if name == "DATABASE_MIGRATION_URL"));
    }

    #[test]
    fn zero_durations_and_limits_are_rejected() {
        for (name, value) in [
            ("API_REQUEST_TIMEOUT_SECS", "0"),
            ("API_BODY_LIMIT_BYTES", "0"),
            ("SHUTDOWN_GRACE_SECS", "0"),
            ("DATABASE_MAX_CONNECTIONS", "0"),
            ("DATABASE_ACQUIRE_TIMEOUT_SECS", "0"),
        ] {
            let env = Env::from_pairs([
                ("API_BIND_ADDR", "127.0.0.1:1"),
                ("DATABASE_URL", DB_URL),
                (name, value),
            ]);
            let err = ApiConfig::from_env(&env).unwrap_err();
            assert!(
                matches!(&err, ConfigError::Invalid { name: n, .. } if n == name),
                "{name}"
            );
        }
    }

    #[test]
    fn value_can_come_from_a_mounted_file() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(file, "127.0.0.1:9000").unwrap();
        let env = Env::from_pairs([
            ("API_BIND_ADDR_FILE", file.path().to_str().unwrap()),
            ("DATABASE_URL", DB_URL),
        ]);
        let cfg = ApiConfig::from_env(&env).unwrap();
        assert_eq!(cfg.bind_addr, "127.0.0.1:9000".parse().unwrap());
    }

    #[test]
    fn unreadable_file_is_reported_with_variable_and_path() {
        let env = Env::from_pairs([("API_BIND_ADDR_FILE", "/nonexistent/secret")]);
        let err = ApiConfig::from_env(&env).unwrap_err();
        assert!(matches!(&err, ConfigError::File { name, .. } if name == "API_BIND_ADDR"));
        assert!(err.to_string().contains("/nonexistent/secret"));
    }

    #[test]
    fn direct_value_and_file_together_are_a_conflict() {
        let env = Env::from_pairs([("API_BIND_ADDR", "127.0.0.1:1"), ("API_BIND_ADDR_FILE", "/tmp/x")]);
        assert!(matches!(
            ApiConfig::from_env(&env),
            Err(ConfigError::Conflict { .. })
        ));
    }

    #[test]
    fn worker_config_has_defaults_and_validates() {
        let cfg = WorkerConfig::from_env(&Env::default()).unwrap();
        assert_eq!(cfg.poll_interval, Duration::from_millis(1000));
        assert_eq!(cfg.health_addr, "0.0.0.0:8081".parse().unwrap());
        let custom = Env::from_pairs([("WORKER_HEALTH_ADDR", "127.0.0.1:9100")]);
        assert_eq!(
            WorkerConfig::from_env(&custom).unwrap().health_addr,
            "127.0.0.1:9100".parse().unwrap()
        );
        let bad = Env::from_pairs([("WORKER_HEALTH_ADDR", "nowhere")]);
        assert!(matches!(
            WorkerConfig::from_env(&bad),
            Err(ConfigError::Invalid { name, .. }) if name == "WORKER_HEALTH_ADDR"
        ));
        let env = Env::from_pairs([("WORKER_POLL_INTERVAL_MS", "abc")]);
        assert!(matches!(
            WorkerConfig::from_env(&env),
            Err(ConfigError::Invalid { .. })
        ));
    }

    #[test]
    fn telemetry_defaults_to_json_logs_without_export() {
        let cfg = TelemetryConfig::from_env(&Env::default()).unwrap();
        assert_eq!(cfg.log_format, LogFormat::Json);
        assert_eq!(cfg.otlp_endpoint, None);

        let text = Env::from_pairs([
            ("LOG_FORMAT", "text"),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://alloy:4318/"),
        ]);
        let cfg = TelemetryConfig::from_env(&text).unwrap();
        assert_eq!(cfg.log_format, LogFormat::Text);
        assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://alloy:4318"));
    }

    #[test]
    fn telemetry_settings_are_validated() {
        let bad_format = Env::from_pairs([("LOG_FORMAT", "xml")]);
        assert!(matches!(
            TelemetryConfig::from_env(&bad_format),
            Err(ConfigError::Invalid { name, .. }) if name == "LOG_FORMAT"
        ));
        let tls = Env::from_pairs([("OTEL_EXPORTER_OTLP_ENDPOINT", "https://otlp.example.org")]);
        assert!(matches!(
            TelemetryConfig::from_env(&tls),
            Err(ConfigError::Invalid { name, .. }) if name == "OTEL_EXPORTER_OTLP_ENDPOINT"
        ));
    }

    #[test]
    fn shared_services_are_optional_and_reduced_to_addresses() {
        assert_eq!(
            SharedServices::from_env(&Env::default()).unwrap(),
            SharedServices::default()
        );
        assert!(SharedServices::default().dependencies().is_empty());

        let both = Env::from_pairs([
            ("VALKEY_URL", "redis://:pw@valkey:6379/0"), // gitleaks:allow (fake test password)
            ("S3_ENDPOINT", "http://seaweedfs:8333"),
        ]);
        let shared = SharedServices::from_env(&both).unwrap();
        assert_eq!(
            shared.valkey.as_deref(),
            Some("valkey:6379"),
            "the password is dropped"
        );
        assert_eq!(shared.s3.as_deref(), Some("seaweedfs:8333"));
        let names: Vec<_> = shared
            .dependencies()
            .iter()
            .map(crate::health::Dependency::name)
            .collect();
        assert_eq!(names, ["valkey", "s3"]);

        let bad = Env::from_pairs([("S3_ENDPOINT", "seaweedfs:8333")]);
        assert!(matches!(
            SharedServices::from_env(&bad),
            Err(ConfigError::Invalid { name, .. }) if name == "S3_ENDPOINT"
        ));
    }

    #[test]
    fn worker_database_is_optional() {
        assert_eq!(WorkerConfig::from_env(&Env::default()).unwrap().database, None);
        let env = Env::from_pairs([("DATABASE_URL", "postgres://app@postgres/mlbpp")]);
        assert!(WorkerConfig::from_env(&env).unwrap().database.is_some());
    }

    #[test]
    fn mcp_config_requires_an_http_base_url() {
        let ok = Env::from_pairs([
            ("MCP_BIND_ADDR", "0.0.0.0:8090"),
            ("MCP_API_BASE_URL", "http://api:8080/"),
        ]);
        assert_eq!(McpConfig::from_env(&ok).unwrap().api_base_url, "http://api:8080");

        let bad = Env::from_pairs([
            ("MCP_BIND_ADDR", "0.0.0.0:8090"),
            ("MCP_API_BASE_URL", "api:8080"),
        ]);
        let err = McpConfig::from_env(&bad).unwrap_err();
        assert!(matches!(&err, ConfigError::Invalid { name, .. } if name == "MCP_API_BASE_URL"));
    }
}
