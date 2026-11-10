//! Valkey connection (sessions, rate limits; later cache and pub/sub).

use std::time::Duration;

use fred::prelude::{Builder, ClientLike, Config, ReconnectPolicy};

use crate::config::Secret;

/// A Valkey client: cheap to clone, reconnects by itself.
pub type Valkey = fred::clients::Client;

/// The Valkey error type.
pub type ValkeyError = fred::error::Error;

/// Connects to `url`, a `redis://` URL (host, port, optional password and
/// database), and waits for the first connection.
pub async fn connect(url: &Secret) -> Result<Valkey, ValkeyError> {
    let client = build(url)?;
    client.init().await?;
    Ok(client)
}

/// Like [`connect`], without waiting: the client connects in the background
/// and commands wait for it (up to their timeout). Lets a service start while
/// Valkey is down; its readiness probe reports it. Needs a Tokio runtime.
pub fn connect_lazy(url: &Secret) -> Result<Valkey, ValkeyError> {
    let client = build(url)?;
    // The connection task runs until the client quits; nothing to await.
    drop(client.connect());
    Ok(client)
}

fn build(url: &Secret) -> Result<Valkey, ValkeyError> {
    let config = Config::from_url(url.expose())?;
    Builder::from_config(config)
        .with_connection_config(|connection| {
            connection.connection_timeout = Duration::from_secs(5);
        })
        .with_performance_config(|performance| {
            performance.default_command_timeout = Duration::from_secs(2);
        })
        // Reconnects forever: 100 ms, doubling, at most 5 s apart.
        .set_policy(ReconnectPolicy::new_exponential(0, 100, 5_000, 2))
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_lazy_client_starts_without_valkey_and_its_commands_fail_fast() {
        use fred::prelude::KeysInterface;
        let client = connect_lazy(&Secret::new("redis://127.0.0.1:1".to_owned())).unwrap();
        let started = std::time::Instant::now();
        let result: Result<Option<String>, _> = client.get("any").await;
        assert!(result.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "{:?}",
            started.elapsed()
        );
    }

    #[test]
    fn malformed_urls_are_rejected() {
        assert!(build(&Secret::new("not a url".to_owned())).is_err());
    }
}
