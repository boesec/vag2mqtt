//! VAG2MQTT service entry point.
//!
//! This binary only wires things together: bootstrap settings, logging, the database, the
//! compiled-in connectors, the supervisor and the management interface. Every rule lives in the
//! crate that owns it.
//!
//! The `debug-login` subcommand is the exception: it touches none of this and runs one login
//! attempt against a manufacturer account (WP-06).

mod cli;
mod debug_login;
mod logging;
mod shutdown;
mod wiring;

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

    wiring::run(&cli).await
}
