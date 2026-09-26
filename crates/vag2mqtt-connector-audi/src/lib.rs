//! Audi connector for VAG2MQTT.
//!
//! The only crate that knows Audi endpoints. A whole way of reaching an account sits behind the
//! [`auth::AudiAuthStrategy`] trait, because the two routes differ in more than their login:
//! they differ in the shape of their session, in their data source kind and in what they can
//! deliver at all. Everything a route does can be recorded as masked trace files through
//! [`TraceConfig`], off by default.
//!
//! Two routes exist:
//!
//! - [`auth::portal::EuDataActStrategy`], the default since 2026-09-24: the official EU Data Act
//!   portal, read only, session held in a cookie jar.
//! - [`auth::form::FormLoginStrategy`], the native myAudi route: its token exchange is refused
//!   for a public client, so it does not reach tokens today. The portal reuses four of its five
//!   steps, which is why it stays.
//!
//! Endpoints, parameters and their sources are documented at the code that uses them.

pub mod auth;
mod connector;
mod cookies;
mod error;
mod export;
mod session;
mod trace;

pub use connector::{
    AudiConnector, MIN_LIVE_POLLING_INTERVAL, MIN_POLLING_INTERVAL, MIN_PORTAL_POLLING_INTERVAL,
};
pub use error::Step;
pub use session::AudiTokens;
pub use trace::{TRACE_ENV, TraceConfig};
