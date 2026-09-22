//! Brand independent vehicle domain model for VAG2MQTT.
//!
//! This crate holds pure data types: accounts, vehicles, the vehicle state snapshot,
//! the error categories and the three state value container from section 4.3 of the
//! requirements (*unsupported* / *temporarily unavailable* / *present*).
//!
//! It performs no I/O. It must not depend on `tokio`, `reqwest`, `sqlx` or `rumqttc`;
//! every other crate in the workspace may depend on it.
//!
//! # Conventions
//!
//! - Units are metric only and encoded in the value types of [`units`]. A connector converts at
//!   its edge; nothing in here converts.
//! - Every vehicle value is a [`Reading`]. There is no `Default`, so a connector has to decide
//!   for each field whether the vehicle lacks the capability, failed to deliver it, or delivered it.
//! - Categorical enums end in an `Unknown` variant for manufacturer strings we cannot name.
//!   `Unknown` is a present value; it does not hide behind [`Reading::Unavailable`].
//! - Secrets travel as [`Secret`], which never prints its content. Coordinates
//!   ([`state::GeoPosition`]) and VINs ([`Vin`]) redact themselves in `Debug`.
//!
//! Implemented by WP-01.

pub mod account;
pub mod config;
pub mod error;
pub mod id;
pub mod reading;
pub mod secret;
mod serde_helpers;
pub mod state;
pub mod units;
pub mod vehicle;

pub use account::{Account, AccountConnectionState, PollingConfig};
pub use config::{MqttConfig, MqttProtocol};
pub use error::{DomainError, ErrorCategory, LastError};
pub use id::{AccountId, Brand, DataSourceKind, Drivetrain, Vin};
pub use reading::{Reading, Sample};
pub use secret::Secret;
pub use state::VehicleState;
pub use units::{HasUnit, Unit};
pub use vehicle::{Vehicle, VehicleDataState};
