//! The `Connector` implementation for Audi.
//!
//! The connector is an adapter and nothing more: the route
//! ([`AudiAuthStrategy`](crate::auth::AudiAuthStrategy)) decides how the account is reached, what
//! a session looks like and what `ConnectorInfo` to report. Since 2026-09-24 the default route
//! is the EU Data Act portal, because both native routes are closed to third parties (see
//! [`crate::auth::portal`]).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use vag2mqtt_connector_api::{
    Connector, ConnectorError, ConnectorInfo, Credentials, DiscoveredVehicle, SessionState,
};
use vag2mqtt_domain::{VehicleState, Vin};

use crate::auth::AudiAuthStrategy;
use crate::auth::form::FormLoginStrategy;
use crate::auth::portal::EuDataActStrategy;
use crate::trace::TraceConfig;

/// The shortest polling interval the native live route tolerates: five minutes, settled on
/// 2026-09-22 to protect the account from being throttled.
pub const MIN_LIVE_POLLING_INTERVAL: Duration = Duration::from_secs(300);

/// The shortest polling interval the EU Data Act portal is worth asking at.
///
/// The portal produces a data package roughly every fifteen minutes and never sooner, so a
/// shorter interval would only cost requests.
pub const MIN_PORTAL_POLLING_INTERVAL: Duration = Duration::from_secs(900);

/// The minimum interval of the default route, kept for callers that ask the crate rather than
/// the connector.
pub const MIN_POLLING_INTERVAL: Duration = MIN_PORTAL_POLLING_INTERVAL;

const LOG: &str = "vag2mqtt::auth";

/// The Audi connector.
pub struct AudiConnector {
    info: ConnectorInfo,
    strategy: Arc<dyn AudiAuthStrategy>,
    trace: TraceConfig,
}

impl std::fmt::Debug for AudiConnector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudiConnector")
            .field("strategy", &self.strategy.name())
            .field("kind", &self.info.kind)
            .field("trace", &self.trace.is_enabled())
            .finish()
    }
}

impl AudiConnector {
    /// The production connector: Audi over the EU Data Act portal.
    pub fn new(trace: TraceConfig) -> Result<Self, ConnectorError> {
        Self::with_strategy(Arc::new(EuDataActStrategy::new()?), trace)
    }

    /// The native live route.
    ///
    /// Its token exchange is refused for a public client, so this is not a working route today.
    /// It stays reachable for a future spike, should the manufacturer reopen it.
    pub fn native(trace: TraceConfig) -> Result<Self, ConnectorError> {
        Self::with_strategy(Arc::new(FormLoginStrategy::new()), trace)
    }

    /// A connector on a specific route.
    pub fn with_strategy(
        strategy: Arc<dyn AudiAuthStrategy>,
        trace: TraceConfig,
    ) -> Result<Self, ConnectorError> {
        Ok(Self {
            info: strategy.info(),
            strategy,
            trace,
        })
    }

    /// The route's name.
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
        let session = self.strategy.login(credentials, &mut trace).await?;
        tracing::info!(target: LOG, strategy = self.strategy.name(), "login succeeded");
        Ok(session)
    }

    async fn refresh(&self, session: &mut SessionState) -> Result<(), ConnectorError> {
        let mut trace = self.trace.begin("refresh");
        self.strategy.refresh(session, &mut trace).await
    }

    async fn list_vehicles(
        &self,
        session: &mut SessionState,
    ) -> Result<Vec<DiscoveredVehicle>, ConnectorError> {
        let mut trace = self.trace.begin("vehicles");
        self.strategy.list_vehicles(session, &mut trace).await
    }

    async fn fetch_state(
        &self,
        session: &mut SessionState,
        vin: &Vin,
    ) -> Result<VehicleState, ConnectorError> {
        let mut trace = self.trace.begin("state");
        self.strategy.fetch_state(session, vin, &mut trace).await
    }
}
