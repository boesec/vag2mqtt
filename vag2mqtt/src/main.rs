//! VAG2MQTT service entry point.
//!
//! This binary only wires things together: it reads the bootstrap settings, installs
//! logging, makes sure the data directory exists and then hands over to the supervisor.
//!
//! As of WP-06 the supervisor, persistence, MQTT publisher and admin UI are built but not
//! wired into the service yet; that happens with WP-08, which needs the same runtime handle for
//! its API. The process therefore starts, reports its configuration and waits for a shutdown
//! signal. The `debug-login` subcommand is the exception: it runs one login attempt against a
//! manufacturer account and exits.

mod cli;
mod debug_login;
mod logging;
mod shutdown;

use std::process::ExitCode;

use anyhow::Context;
use anyhow::Result;
use clap::Parser;

use crate::cli::{Cli, Command};

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

    if let Some(Command::DebugLogin(args)) = cli.command {
        return debug_login::run(args).await;
    }

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
