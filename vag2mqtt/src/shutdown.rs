//! Graceful shutdown.
//!
//! The service is meant to run continuously (requirement NFR-002), so it stops only on an
//! explicit signal: Ctrl-C everywhere, and additionally SIGTERM on Unix, which is what a
//! container runtime or systemd sends.

use anyhow::Context;
use anyhow::Result;

/// Waits until the process is asked to stop.
///
/// # Errors
///
/// Fails if the signal handlers cannot be installed.
pub(crate) async fn wait_for_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::SignalKind;
        use tokio::signal::unix::signal;

        let mut terminate =
            signal(SignalKind::terminate()).context("failed to install the SIGTERM handler")?;
        let mut interrupt =
            signal(SignalKind::interrupt()).context("failed to install the SIGINT handler")?;

        tokio::select! {
            _ = terminate.recv() => Ok(()),
            _ = interrupt.recv() => Ok(()),
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .context("failed to wait for Ctrl-C")
    }
}
