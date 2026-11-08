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

/// Configuration of the `api` service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiConfig {
    pub common: CommonConfig,
    pub bind_addr: SocketAddr,
    pub request_timeout: Duration,
    pub body_limit_bytes: usize,
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
        })
    }
}

/// Configuration of the `worker` service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerConfig {
    pub common: CommonConfig,
    /// Pause between two polls of the job queue when it is empty.
    pub poll_interval: Duration,
}

impl WorkerConfig {
    pub fn from_env(env: &Env) -> Result<Self, ConfigError> {
        Ok(Self {
            common: CommonConfig::from_env(env)?,
            poll_interval: env.millis("WORKER_POLL_INTERVAL_MS", 1000)?,
        })
    }
}

/// Configuration of the `mcp` service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfig {
    pub common: CommonConfig,
    pub bind_addr: SocketAddr,
    /// Base URL of the public API the MCP server calls, e.g. `http://api:8080`.
    pub api_base_url: String,
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

    #[test]
    fn defaults_apply_when_optional_values_are_absent() {
        let env = Env::from_pairs([("API_BIND_ADDR", "0.0.0.0:8080")]);
        let cfg = ApiConfig::from_env(&env).unwrap();
        assert_eq!(cfg.bind_addr, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(cfg.request_timeout, Duration::from_secs(30));
        assert_eq!(cfg.body_limit_bytes, 2 * 1024 * 1024);
        assert_eq!(cfg.common.shutdown_grace, Duration::from_secs(30));
    }

    #[test]
    fn zero_durations_and_limits_are_rejected() {
        for (name, value) in [
            ("API_REQUEST_TIMEOUT_SECS", "0"),
            ("API_BODY_LIMIT_BYTES", "0"),
            ("SHUTDOWN_GRACE_SECS", "0"),
        ] {
            let env = Env::from_pairs([("API_BIND_ADDR", "127.0.0.1:1"), (name, value)]);
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
        let env = Env::from_pairs([("API_BIND_ADDR_FILE", file.path().to_str().unwrap())]);
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
        let env = Env::from_pairs([("WORKER_POLL_INTERVAL_MS", "abc")]);
        assert!(matches!(
            WorkerConfig::from_env(&env),
            Err(ConfigError::Invalid { .. })
        ));
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
