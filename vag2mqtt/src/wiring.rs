//! Putting the crates together.
//!
//! The only place that knows every crate at once. It opens the database, builds the connector
//! registry, starts the supervisor and serves the management interface, then shuts all three
//! down in order when a signal arrives.

use std::sync::Arc;

use anyhow::{Context, Result};
use vag2mqtt_admin::AppState;
use vag2mqtt_connector_api::ConnectorRegistry;
use vag2mqtt_connector_audi::{AudiConnector, TraceConfig};
use vag2mqtt_domain::Secret;
use vag2mqtt_mqtt::ServiceInfo;
use vag2mqtt_persistence::{Database, MasterKeySource};
use vag2mqtt_runtime::{RuntimeDeps, Supervisor, SystemClock, mqtt_publisher_factory};

use crate::cli::Cli;
use crate::shutdown;

/// The environment variable the master key may come from.
const MASTER_KEY_ENV: &str = "VAG2MQTT_MASTER_KEY";

const LOG: &str = "vag2mqtt::app";

/// Starts everything and runs until a shutdown signal.
pub(crate) async fn run(cli: &Cli) -> Result<()> {
    let db = Database::open(&cli.data_dir, master_key_source())
        .await
        .context("failed to open the database")?;
    if let Some(path) = db.key_file_path() {
        tracing::info!(target: LOG, path = %path.display(), "master key read from the key file");
    } else {
        tracing::info!(target: LOG, "master key read from {MASTER_KEY_ENV}");
    }

    let registry = build_registry().context("failed to build the connector registry")?;
    let brands = registry.brands();
    tracing::info!(target: LOG, ?brands, "compiled-in connectors");

    let service_info = ServiceInfo {
        name: "vag2mqtt".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        started_at: chrono::Utc::now(),
    };

    let (runtime, supervisor_task) = Supervisor::start(RuntimeDeps {
        db,
        registry,
        publisher_factory: mqtt_publisher_factory(service_info),
        clock: Arc::new(SystemClock),
        settings: None,
    })
    .await
    .context("failed to start the runtime")?;

    let state = AppState::new(runtime.clone(), brands);
    let serve = vag2mqtt_admin::serve(cli.listen, state, async {
        if let Err(error) = shutdown::wait_for_signal().await {
            tracing::error!(target: LOG, %error, "could not listen for a shutdown signal");
        }
        tracing::info!(target: LOG, "shutdown signal received, stopping");
    });

    let result = serve.await.context("the management interface stopped");

    // The API is down; stop the runtime before the process leaves, so the MQTT publisher gets to
    // announce that it is gone rather than leaving the last will to do it.
    if let Err(error) = runtime.shutdown().await {
        tracing::warn!(target: LOG, %error, "the runtime did not confirm its shutdown");
    }
    supervisor_task.abort();

    tracing::info!(target: LOG, "stopped");
    result
}

/// Where the master key comes from: the environment when set, else the key file.
fn master_key_source() -> MasterKeySource {
    match std::env::var(MASTER_KEY_ENV) {
        Ok(key) if !key.trim().is_empty() => MasterKeySource::Provided(Secret::new(key)),
        _ => MasterKeySource::KeyFile,
    }
}

/// The connectors this build ships. The only function that names a brand crate: everything
/// else works through the `Connector` trait.
fn build_registry() -> Result<ConnectorRegistry> {
    let audi = AudiConnector::new(TraceConfig::from_env())
        .map_err(|error| anyhow::anyhow!("{error}"))
        .context("failed to build the Audi connector")?;
    let registry = ConnectorRegistry::new()
        .register(Arc::new(audi))
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    Ok(registry)
}
