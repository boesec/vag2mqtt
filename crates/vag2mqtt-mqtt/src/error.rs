//! Errors of the MQTT layer.

/// Everything that can go wrong in this crate. Never carries a password.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MqttError {
    /// The configured topic prefix is not usable.
    #[error("invalid topic prefix: {reason}")]
    InvalidPrefix {
        /// Why it was rejected.
        reason: &'static str,
    },
    /// A topic segment (VIN, account id, path) contains characters MQTT does not allow.
    #[error("invalid topic segment: {reason}")]
    InvalidSegment {
        /// Why it was rejected.
        reason: &'static str,
    },
    /// The client's outgoing queue is full; the caller should drop this message and log.
    #[error("mqtt client queue is full, message dropped")]
    Backpressure,
    /// The publisher task has stopped; the handle is stale.
    #[error("mqtt publisher is not running")]
    Stopped,
    /// A payload could not be serialised.
    #[error("could not serialise payload for {topic}")]
    Serialisation {
        /// The topic that was being published.
        topic: String,
    },
}
