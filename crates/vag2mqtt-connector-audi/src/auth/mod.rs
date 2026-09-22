//! Authentication strategies. One trait, several routes; the name of the route that produced a
//! session is stored with it.

pub mod form;
mod http;

use async_trait::async_trait;
use vag2mqtt_connector_api::{ConnectorError, Credentials};

use crate::session::AudiTokens;
use crate::trace::Trace;

pub use http::HttpClient;

/// One way of obtaining myAudi tokens.
#[async_trait]
pub trait AudiAuthStrategy: Send + Sync {
    /// The route's name, stored in the session: `form`, later `device_code`.
    fn name(&self) -> &'static str;

    /// Obtains fresh tokens from credentials.
    async fn login(
        &self,
        http: &HttpClient,
        credentials: &Credentials,
        trace: &mut Trace,
    ) -> Result<AudiTokens, ConnectorError>;

    /// Renews tokens. A rejection means the session is gone and a login is needed.
    async fn refresh(
        &self,
        http: &HttpClient,
        tokens: &AudiTokens,
        trace: &mut Trace,
    ) -> Result<AudiTokens, ConnectorError>;
}
