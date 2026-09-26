//! Supervisor, per account tasks and polling for VAG2MQTT.
//!
//! [`Supervisor::start`] loads accounts from the database, spawns one isolated task per enabled
//! account and the MQTT publisher, and returns a [`RuntimeHandle`]. The management API
//! drives everything through the handle's commands and reads the [`RuntimeStatus`] snapshot;
//! it never touches an account task directly.
//!
//! Every account task authenticates, discovers vehicles, polls them sequentially, persists the
//! snapshot and publishes it. Failures are classified through
//! [`ConnectorError::class`](vag2mqtt_connector_api::ConnectorError::class) and handled per
//! [`ErrorClass`](vag2mqtt_connector_api::ErrorClass): stop on bad credentials, log in again on
//! an expired session, pause on a rate limit, back off on anything transient.

mod account_task;
mod backoff;
mod clock;
mod command;
mod error;
mod publisher;
mod settings;
mod status;
mod supervisor;

pub use clock::{Clock, ManualClock, SystemClock};
pub use command::{AccountCreate, AccountUpdate, RuntimeHandle};
pub use error::RuntimeError;
pub use publisher::{Publisher, PublisherFactory, StatePublishReport, mqtt_publisher_factory};
pub use settings::RuntimeSettings;
pub use status::{AccountStatus, MqttStatus, RuntimeStatus, VehicleStatus};
pub use supervisor::{RuntimeDeps, Supervisor};
