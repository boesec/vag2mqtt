//! mqtt-smarthome publisher for VAG2MQTT.
//!
//! - [`TopicBuilder`]: the single place where topic strings come into existence.
//! - [`flatten`]: turns a [`VehicleState`](vag2mqtt_domain::VehicleState) into the scalar
//!   `status/<VIN>/<path>` values of contract version 1 (`Docs/mqtt-contract.md`).
//! - [`LastChangeTracker`]: keeps `lc` per topic, seedable from persistence.
//! - [`MqttPublisher`] / [`MqttHandle`]: the rumqttc client task and the cheap handle the runtime
//!   publishes through.
//!
//! Wire conventions follow mqtt-smarthome 2.0 (`Docs/reference/mqtt-smarthome.md`): `connected`
//! tri-state with a last will of `0`, retained `status/*` with `{"val","ts","lc"}` where `ts`
//! and `lc` are milliseconds since the epoch, and `info` with `name`, `version` and `spec`.
//!
//! Implemented by WP-04.

mod error;
mod flatten;
mod last_change;
mod payload;
mod publisher;
mod topic;

pub use error::MqttError;
pub use flatten::{FlatValue, PATH_PATTERNS, door_segment, flatten, pattern_of, window_segment};
pub use last_change::LastChangeTracker;
pub use payload::{
    CONTRACT_VERSION, FullPayload, InfoPayload, SPEC, ServiceInfo, StatePayload, VehicleMeta, to_ms,
};
pub use publisher::{Incoming, MqttHandle, MqttPublisher, PublishReport};
pub use topic::TopicBuilder;
