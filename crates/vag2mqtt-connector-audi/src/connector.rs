//! The `Connector` implementation for Audi.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use vag2mqtt_connector_api::{
    Connector, ConnectorError, ConnectorInfo, Credentials, DiscoveredVehicle, SessionState,
};
use vag2mqtt_domain::{Brand, DataSourceKind, VehicleState, Vin};

use crate::auth::form::FormLoginStrategy;
use crate::auth::{AudiAuthStrategy, HttpClient};
use crate::session::AudiTokens;
use crate::trace::TraceConfig;

/// The shortest polling interval the connector tolerates: five minutes, settled on 2026-09-22 to
/// protect the account (`Docs/reference/audi-auth.md` section 6).
pub const MIN_POLLING_INTERVAL: Duration = Duration::from_secs(300);

const LOG: &str = "vag2mqtt::auth";

/// The Audi connector.
pub struct AudiConnector {
    info: ConnectorInfo,
    http: HttpClient,
    strategy: Arc<dyn AudiAuthStrategy>,
    trace: TraceConfig,
}

impl std::fmt::Debug for AudiConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudiConnector")
            .field("strategy", &self.strategy.name())
            .field("trace", &self.trace.is_enabled())
            .finish()
    }
}

impl AudiConnector {
    /// The production connector with the form login.
    pub fn new(trace: TraceConfig) -> Result<Self, ConnectorError> {
        Self::with_strategy(Arc::new(FormLoginStrategy::new()), trace)
    }

    /// A connector with a specific authentication strategy.
    pub fn with_strategy(
        strategy: Arc<dyn AudiAuthStrategy>,
        trace: TraceConfig,
    ) -> Result<Self, ConnectorError> {
        Ok(Self {
            info: ConnectorInfo {
                brand: Brand::Audi,
                kind: DataSourceKind::LiveApi,
                min_polling_interval: MIN_POLLING_INTERVAL,
                supports_commands: false,
            },
            http: HttpClient::new()?,
            strategy,
            trace,
        })
    }

    /// The strategy's name.
    pub fn strategy_name(&self) -> &'static str {
        self.strategy.name()
    }
}

#[async_trait]
impl Connector for AudiConnector {
    fn info(&self) -> &ConnectorInfo {
        &self.info
    }

    async fn login(&self, credentials: &Credentials) -> Result<SessionState, ConnectorError> {
        let mut trace = self.trace.begin(&credentials.username);
        tracing::info!(target: LOG, strategy = self.strategy.name(), "logging in");
        let tokens = self
            .strategy
            .login(&self.http, credentials, &mut trace)
            .await?;
        tracing::info!(target: LOG, expires_at = %tokens.expires_at, "login succeeded");
        Ok(tokens.into_session())
    }

    async fn refresh(&self, session: &mut SessionState) -> Result<(), ConnectorError> {
        let tokens = AudiTokens::from_session(session)?;
        let mut trace = self.trace.begin("refresh");
        let renewed = self
            .strategy
            .refresh(&self.http, &tokens, &mut trace)
            .await?;
        *session = renewed.into_session();
        Ok(())
    }

    async fn list_vehicles(
        &self,
        _session: &mut SessionState,
    ) -> Result<Vec<DiscoveredVehicle>, ConnectorError> {
        Err(ConnectorError::Unsupported {
            operation: "list_vehicles (WP-07)",
        })
    }

    async fn fetch_state(
        &self,
        _session: &mut SessionState,
        _vin: &Vin,
    ) -> Result<VehicleState, ConnectorError> {
        Err(ConnectorError::Unsupported {
            operation: "fetch_state (WP-07)",
        })
    }
}
