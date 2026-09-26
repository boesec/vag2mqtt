//! Bootstrap settings.
//!
//! These are the only settings that live outside the database. Everything else (accounts,
//! vehicles, MQTT broker, polling) is configured at runtime through the admin UI or API,
//! so the service starts on an empty data directory without any configuration file
//!.
//!
//! The master key is deliberately **not** a command line flag: as an argument it would show
//! up in `--help`, in the process list and in any `Debug` output. It is read from the
//! `VAG2MQTT_MASTER_KEY` environment variable or the key file instead.

use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;

/// Command line and environment configuration for the service.
#[derive(Debug, Parser)]
#[command(
    name = "vag2mqtt",
    version,
    about = "Connect Volkswagen Group vehicles to MQTT",
    after_help = "Read from the environment only:\n  VAG2MQTT_MASTER_KEY  Key used to \
                  encrypt stored credentials and tokens. When unset, a key file is created \
                  in the data directory on first start."
)]
pub(crate) struct Cli {
    /// Directory holding the database, the master key and other runtime state.
    #[arg(
        long,
        env = "VAG2MQTT_DATA_DIR",
        default_value = "./data",
        value_name = "DIR"
    )]
    pub(crate) data_dir: PathBuf,

    /// Address the admin API and UI listen on.
    #[arg(
        long,
        env = "VAG2MQTT_LISTEN",
        default_value = "127.0.0.1:8080",
        value_name = "ADDR"
    )]
    pub(crate) listen: SocketAddr,

    /// Log filter in `tracing` EnvFilter syntax, for example `info,vag2mqtt::mqtt=debug`.
    #[arg(
        long,
        env = "VAG2MQTT_LOG",
        default_value = "info",
        value_name = "FILTER"
    )]
    pub(crate) log: String,

    /// Log output format.
    #[arg(
        long,
        env = "VAG2MQTT_LOG_FORMAT",
        default_value = "pretty",
        value_name = "FORMAT"
    )]
    pub(crate) log_format: LogFormat,

    /// Diagnostic subcommand. Without one, the service runs.
    #[command(subcommand)]
    pub(crate) command: Option<Command>,
}

/// Diagnostic subcommands.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Run one login attempt against a manufacturer account and report where it gets to.
    ///
    /// Touches neither the database nor MQTT. Exactly one attempt per invocation, so it cannot
    /// walk an account into a lockout. Temporary: it goes away once the management interface can
    /// do the same.
    DebugLogin(DebugLogin),
}

/// Arguments of `debug-login`.
#[derive(Debug, clap::Args)]
pub(crate) struct DebugLogin {
    /// The manufacturer service to log in to.
    #[arg(long, value_name = "BRAND", default_value = "audi")]
    pub(crate) brand: DebugBrand,

    /// The account's user name, usually an e-mail address.
    #[arg(long, value_name = "USER")]
    pub(crate) username: String,

    /// Write masked request and response files into this directory. Also settable through
    /// `VAG2MQTT_AUDI_TRACE`.
    #[arg(long, value_name = "DIR")]
    pub(crate) trace_dir: Option<PathBuf>,
}

/// Brands `debug-login` can try.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum DebugBrand {
    /// myAudi.
    Audi,
}

/// How log lines are rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum LogFormat {
    /// Human readable output for a terminal.
    Pretty,
    /// One JSON object per line, for log collectors.
    Json,
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_match_the_documented_bootstrap_settings() {
        let cli = Cli::parse_from(["vag2mqtt"]);

        assert_eq!(cli.data_dir, PathBuf::from("./data"));
        assert_eq!(cli.listen.to_string(), "127.0.0.1:8080");
        assert_eq!(cli.log, "info");
        assert_eq!(cli.log_format, LogFormat::Pretty);
    }

    #[test]
    fn master_key_is_not_exposed_as_an_argument() {
        let has_master_key_argument = Cli::command()
            .get_arguments()
            .any(|argument| argument.get_id().as_str().contains("master"));

        assert!(
            !has_master_key_argument,
            "the master key must stay out of the argument list"
        );
    }
}
