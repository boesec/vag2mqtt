//! Authentication routes. One trait, several routes; the name of the route that produced a
//! session is stored with it.
//!
//! A route is more than a login: the native route ends at a bearer token and a live API, the EU
//! Data Act portal ends at a session cookie and an export. They therefore differ in the shape of
//! their session, in what `ConnectorInfo` they report, and in how vehicles are discovered. The
//! trait covers all of that so that [`AudiConnector`](crate::AudiConnector) stays an adapter.

pub mod form;
mod http;
pub mod portal;

use async_trait::async_trait;
use vag2mqtt_connector_api::{
    ConnectorError, ConnectorInfo, Credentials, DiscoveredVehicle, SessionState,
};
use vag2mqtt_domain::{VehicleState, Vin};

use crate::trace::Trace;

pub use http::HttpClient;

/// One way of reaching an Audi account.
#[async_trait]
pub trait AudiAuthStrategy: Send + Sync {
    /// The route's name, stored in the session: `form` or `eu_data_act`.
    fn name(&self) -> &'static str;

    /// What the connector reports about this route: data source kind, minimum interval and
    /// whether commands are possible.
    fn info(&self) -> ConnectorInfo;

    /// Obtains a session from credentials.
    ///
    /// The route builds its own HTTP client, because a session is a cookie jar on one route and
    /// a token on the other, and a client shared between accounts would mix the two up.
    async fn login(
        &self,
        credentials: &Credentials,
        trace: &mut Trace,
    ) -> Result<SessionState, ConnectorError>;

    /// Renews the session in place. A rejection means the session is gone and a login is needed.
    async fn refresh(
        &self,
        session: &mut SessionState,
        trace: &mut Trace,
    ) -> Result<(), ConnectorError>;

    /// The vehicles the account has, if the route can ask.
    async fn list_vehicles(
        &self,
        _session: &mut SessionState,
        _trace: &mut Trace,
    ) -> Result<Vec<DiscoveredVehicle>, ConnectorError> {
        Err(ConnectorError::Unsupported {
            operation: "list_vehicles on this route",
        })
    }

    /// One vehicle's state, if the route can fetch it.
    async fn fetch_state(
        &self,
        _session: &mut SessionState,
        _vin: &Vin,
        _trace: &mut Trace,
    ) -> Result<VehicleState, ConnectorError> {
        Err(ConnectorError::Unsupported {
            operation: "fetch_state on this route",
        })
    }
}
