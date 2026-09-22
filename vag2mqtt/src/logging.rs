//! Logging setup.
//!
//! Log targets follow the categories from requirement FR-022, so a filter such as
//! `info,vag2mqtt::auth=debug` turns up the detail for one area only.
//!
//! Secrets, full VINs and coordinates must never reach a log line. The redaction layer
//! that enforces this arrives with WP-10; until then the rule is upheld by hand.

use anyhow::Context;
use anyhow::Result;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::prelude::*;

use crate::cli::LogFormat;

/// Installs the global tracing subscriber.
///
/// # Errors
///
/// Fails if the filter cannot be parsed or a subscriber is already installed.
pub(crate) fn init(filter: &str, format: LogFormat) -> Result<()> {
    let env_filter =
        EnvFilter::try_new(filter).with_context(|| format!("invalid log filter `{filter}`"))?;

    let registry = tracing_subscriber::registry().with(env_filter);

    match format {
        LogFormat::Pretty => registry.with(fmt::layer()).try_init(),
        LogFormat::Json => registry.with(fmt::layer().json()).try_init(),
    }
    .context("failed to install the tracing subscriber")
}
