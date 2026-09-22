//! VAG2MQTT service entry point.
//!
//! This binary only wires things together: it reads the bootstrap settings, installs
//! logging, makes sure the data directory exists and then hands over to the supervisor.
//!
//! As of WP-00 the supervisor, persistence, MQTT publisher and admin UI are still empty
//! skeletons, so the process starts, reports its configuration and waits for a shutdown
//! signal.

mod cli;
mod logging;
mod shutdown;

use std::process::ExitCode;

use anyhow::Context;
use anyhow::Result;
use clap::Parser;

use crate::cli::Cli;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // Logging may not be installed yet, so report on stderr as well.
            eprintln!("vag2mqtt failed: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    logging::init(&cli.log, cli.log_format).context("failed to set up logging")?;

    std::fs::create_dir_all(&cli.data_dir)
        .with_context(|| format!("failed to create data directory {}", cli.data_dir.display()))?;

    tracing::info!(
        target: "vag2mqtt::app",
        version = env!("CARGO_PKG_VERSION"),
        data_dir = %cli.data_dir.display(),
        listen = %cli.listen,
        log_filter = %cli.log,
        "vag2mqtt starting"
    );
    tracing::warn!(
        target: "vag2mqtt::app",
        "this build is the WP-00 skeleton: no accounts, no MQTT and no admin interface yet"
    );

    shutdown::wait_for_signal().await?;

    tracing::info!(target: "vag2mqtt::app", "shutdown signal received, stopping");

    Ok(())
}
