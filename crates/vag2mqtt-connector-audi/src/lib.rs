//! Audi connector for VAG2MQTT.
//!
//! The only crate that knows myAudi endpoints. Authentication sits behind the
//! [`auth::AudiAuthStrategy`] trait so that a second route (device code login) can be added
//! beside the classic form login instead of replacing its insides. Everything the flow does can
//! be recorded as masked trace files through [`TraceConfig`], off by default.
//!
//! Endpoints, parameters and their sources are documented in `Docs/reference/audi-auth.md`.
//!
//! WP-06 implements `login` and `refresh`; `list_vehicles` and `fetch_state` arrive with WP-07.

pub mod auth;
mod connector;
mod error;
mod session;
mod trace;

pub use connector::{AudiConnector, MIN_POLLING_INTERVAL};
pub use error::Step;
pub use session::AudiTokens;
pub use trace::{TRACE_ENV, TraceConfig};
