//! Errors of the runtime.

use std::time::Duration;

use vag2mqtt_domain::Brand;
use vag2mqtt_mqtt::MqttError;
use vag2mqtt_persistence::PersistenceError;

/// What a command or the supervisor can fail with.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    /// The database failed.
    #[error(transparent)]
    Persistence(#[from] PersistenceError),
    /// The MQTT layer failed.
    #[error(transparent)]
    Mqtt(#[from] MqttError),
    /// No connector is compiled in for the brand.
    #[error("no connector for brand {0}")]
    UnknownBrand(Brand),
    /// The polling interval is below what the connector tolerates.
    #[error("polling interval below the connector minimum of {minimum:?}")]
    IntervalBelowMinimum {
        /// The connector's minimum.
        minimum: Duration,
    },
    /// The account or vehicle does not exist.
    #[error("{entity} not found")]
    NotFound {
        /// `account` or `vehicle`.
        entity: &'static str,
    },
    /// Input that does not parse or violates a rule.
    #[error("invalid input: {reason}")]
    Invalid {
        /// What was wrong.
        reason: String,
    },
    /// The supervisor is no longer running.
    #[error("the runtime has stopped")]
    Stopped,
}

impl From<vag2mqtt_domain::DomainError> for RuntimeError {
    fn from(error: vag2mqtt_domain::DomainError) -> Self {
        RuntimeError::Invalid {
            reason: error.to_string(),
        }
    }
}
