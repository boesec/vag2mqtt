//! The `Connector` trait and the data that crosses it.

use std::time::Duration;

use async_trait::async_trait;
use vag2mqtt_domain::{Brand, DataSourceKind, Drivetrain, Secret, VehicleState, Vin};

use crate::error::ConnectorError;
use crate::session::SessionState;

/// A user's login to a manufacturer service.
#[derive(Clone, Debug)]
pub struct Credentials {
    /// The login identifier, usually an email address.
    pub username: String,
    /// The password. Redacted in `Debug`.
    pub password: Secret<String>,
}

/// Static facts about a connector that the runtime and the UI need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorInfo {
    /// The brand this connector serves.
    pub brand: Brand,
    /// How it obtains data (requirements section 4.4).
    pub kind: DataSourceKind,
    /// The shortest polling interval the manufacturer tolerates. The runtime and the UI refuse
    /// anything smaller.
    pub min_polling_interval: Duration,
    /// Whether commands towards the vehicle are possible. `false` for every connector today.
    pub supports_commands: bool,
}

/// What discovery learned about one vehicle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredVehicle {
    /// The VIN.
    pub vin: Vin,
    /// The manufacturer's model designation, if delivered.
    pub model: Option<String>,
    /// The propulsion, derived from the manufacturer's car type.
    pub drivetrain: Drivetrain,
}

/// One brand's implementation.
///
/// Rules every implementation follows:
///
/// - It is stateless apart from its HTTP client. Everything per account lives in the
///   [`SessionState`] the runtime passes in, and a connector may refresh tokens inside that state
///   during any call. The runtime persists the state after every call that returns `Ok`.
/// - [`login`](Connector::login) is both "validate credentials" and "authenticate";
///   the manufacturers offer no separate validation.
/// - [`fetch_state`](Connector::fetch_state) starts from
///   [`VehicleState::unsupported`] and fills what the manufacturer delivers. A category the
///   vehicle lacks stays `Unsupported`; one that is declared but empty becomes `Unavailable`.
/// - Spans carry `brand` and `vin = vin.short()`. Bodies are logged at `trace` only, with tokens
///   redacted. No URL, header, body or token ever reaches an error's `Display`.
#[async_trait]
pub trait Connector: Send + Sync {
    /// Static facts about this connector.
    fn info(&self) -> &ConnectorInfo;

    /// Authenticates with the manufacturer and returns a fresh session.
    async fn login(&self, credentials: &Credentials) -> Result<SessionState, ConnectorError>;

    /// Renews the session's tokens in place.
    async fn refresh(&self, session: &mut SessionState) -> Result<(), ConnectorError>;

    /// Lists the vehicles assigned to the account.
    async fn list_vehicles(
        &self,
        session: &mut SessionState,
    ) -> Result<Vec<DiscoveredVehicle>, ConnectorError>;

    /// Fetches the current state of one vehicle.
    async fn fetch_state(
        &self,
        session: &mut SessionState,
        vin: &Vin,
    ) -> Result<VehicleState, ConnectorError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_debug_redacts_the_password() {
        let credentials = Credentials {
            username: "driver@example.test".into(),
            password: Secret::new("hunter2".into()),
        };
        let debug = format!("{credentials:?}");
        assert!(debug.contains("driver@example.test"));
        assert!(!debug.contains("hunter2"));
    }
}
