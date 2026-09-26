//! The interface every brand connector of VAG2MQTT is written against.
//!
//! - [`Connector`]: the trait. Object safe through `async-trait`, so the runtime holds
//!   `Arc<dyn Connector>` and never names a brand crate.
//! - [`SessionState`]: everything a connector needs between calls, owned by the runtime and
//!   stored encrypted by persistence. Connectors are stateless apart from their HTTP client.
//! - [`ConnectorError`]: one error type with a [`class`](ConnectorError::class) the supervisor
//!   branches on and a [`category`](ConnectorError::category) the user sees.
//! - [`ConnectorRegistry`]: the compiled-in brands, filled by the binary.
//! - [`fake::FakeConnector`] (feature `fake`): a scripted connector for tests.
//!

mod connector;
mod error;
#[cfg(feature = "fake")]
pub mod fake;
mod registry;
mod session;

pub use connector::{Connector, ConnectorInfo, Credentials, DiscoveredVehicle};
pub use error::{ConnectorError, ErrorClass};
pub use registry::{ConnectorRegistry, DuplicateBrand};
pub use session::SessionState;

/// Result alias for connector calls.
pub type Result<T> = std::result::Result<T, ConnectorError>;
