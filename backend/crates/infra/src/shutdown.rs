//! Process signals that trigger a graceful shutdown.
//!
//! Call [`listen`] first thing at startup: from then on, SIGTERM (Docker,
//! Kubernetes) and Ctrl+C (local development) are kept for
//! [`Shutdown::wait`] instead of killing the process. A signal received
//! while the service starts (connecting, checking its storage…) is not
//! lost: the wait returns at once.

#[cfg(unix)]
use tokio::signal::unix::{Signal, SignalKind, signal as unix_signal};

/// The shutdown signals, listened to since [`listen`].
pub struct Shutdown {
    #[cfg(unix)]
    terminate: Option<Signal>,
    #[cfg(unix)]
    interrupt: Option<Signal>,
}

impl std::fmt::Debug for Shutdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shutdown").finish_non_exhaustive()
    }
}

/// Starts listening for SIGTERM and Ctrl+C now. Must run inside the Tokio
/// runtime.
pub fn listen() -> Shutdown {
    #[cfg(unix)]
    {
        let listen_to = |kind: SignalKind, name: &str| match unix_signal(kind) {
            Ok(stream) => Some(stream),
            Err(err) => {
                tracing::error!(error = %err, "cannot listen for {name}");
                None
            }
        };
        Shutdown {
            terminate: listen_to(SignalKind::terminate(), "SIGTERM"),
            interrupt: listen_to(SignalKind::interrupt(), "Ctrl+C"),
        }
    }
    #[cfg(not(unix))]
    {
        Shutdown {}
    }
}

impl Shutdown {
    /// Resolves when a shutdown signal was received since [`listen`].
    pub async fn wait(self) {
        #[cfg(unix)]
        {
            let (mut terminate, mut interrupt) = (self.terminate, self.interrupt);
            let terminate = async {
                match terminate.as_mut() {
                    Some(stream) => {
                        stream.recv().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            };
            let interrupt = async {
                match interrupt.as_mut() {
                    Some(stream) => {
                        stream.recv().await;
                    }
                    None => std::future::pending::<()>().await,
                }
            };
            tokio::select! {
                () = terminate => tracing::info!("SIGTERM received, shutting down"),
                () = interrupt => tracing::info!("Ctrl+C received, shutting down"),
            }
        }
        #[cfg(not(unix))]
        {
            if let Err(err) = tokio::signal::ctrl_c().await {
                tracing::error!(error = %err, "cannot listen for Ctrl+C");
                std::future::pending::<()>().await;
            }
            tracing::info!("Ctrl+C received, shutting down");
        }
    }
}

/// Resolves when the process receives SIGTERM or Ctrl+C, listening from its
/// first poll. Prefer [`listen`] at startup.
pub async fn signal() {
    listen().wait().await;
}
