//! Service configuration records that live in the database and are edited at runtime.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// The MQTT broker connection.
///
/// The password is deliberately not a field: it travels next to the config as a
/// [`Secret<String>`](crate::Secret), so this struct stays serialisable and can be shown in the
/// UI and the API without redaction work.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MqttConfig {
    /// Whether the publisher should connect at all.
    pub enabled: bool,
    /// Broker host name or IP address.
    pub host: String,
    /// Broker port, usually 1883 or 8883 with TLS.
    pub port: u16,
    /// Broker user name, if the broker requires authentication.
    pub username: Option<String>,
    /// Connect with TLS.
    pub tls: bool,
    /// The first topic segment, `vag2mqtt` by default.
    pub topic_prefix: String,
    /// The MQTT client identifier.
    pub client_id: String,
    /// Keep alive interval, serialised as whole seconds under `keep_alive_secs`.
    #[serde(
        rename = "keep_alive_secs",
        with = "crate::serde_helpers::duration_secs"
    )]
    pub keep_alive: Duration,
    /// Protocol version.
    pub protocol: MqttProtocol,
}

impl MqttConfig {
    /// The default topic prefix.
    pub const DEFAULT_PREFIX: &'static str = "vag2mqtt";
    /// The default plain port.
    pub const DEFAULT_PORT: u16 = 1883;
    /// The default keep alive.
    pub const DEFAULT_KEEP_ALIVE: Duration = Duration::from_secs(30);

    /// A configuration with the default settings for the given broker.
    ///
    /// The caller supplies the client identifier because the domain has no randomness; the
    /// admin layer appends a short random suffix to `vag2mqtt-`.
    pub fn defaults(host: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            enabled: true,
            host: host.into(),
            port: Self::DEFAULT_PORT,
            username: None,
            tls: false,
            topic_prefix: Self::DEFAULT_PREFIX.to_string(),
            client_id: client_id.into(),
            keep_alive: Self::DEFAULT_KEEP_ALIVE,
            protocol: MqttProtocol::V3_1_1,
        }
    }
}

/// The MQTT protocol version to speak.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MqttProtocol {
    /// MQTT 3.1.1, the default.
    #[serde(rename = "v3_1_1")]
    V3_1_1,
    /// MQTT 5.
    #[serde(rename = "v5")]
    V5,
}

impl MqttProtocol {
    /// Every variant, in declaration order.
    pub const ALL: &'static [MqttProtocol] = &[MqttProtocol::V3_1_1, MqttProtocol::V5];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_fr_014() {
        let config = MqttConfig::defaults("broker.local", "vag2mqtt-abcd1234");
        assert_eq!(config.port, 1883);
        assert_eq!(config.topic_prefix, "vag2mqtt");
        assert_eq!(config.keep_alive, Duration::from_secs(30));
        assert_eq!(config.protocol, MqttProtocol::V3_1_1);
        assert!(config.enabled);
        assert!(!config.tls);
    }

    #[test]
    fn serde_round_trip_uses_keep_alive_secs() {
        let config = MqttConfig::defaults("broker.local", "vag2mqtt-abcd1234");
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["keep_alive_secs"], serde_json::json!(30));
        assert_eq!(json["protocol"], serde_json::json!("v3_1_1"));
        let back: MqttConfig = serde_json::from_value(json).unwrap();
        assert_eq!(back, config);
    }
}
