//! Payload shapes of the contract.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use vag2mqtt_domain::{Brand, VehicleState, Vin};

/// The version of the MQTT contract this crate implements (`CONTRACT.md`).
pub const CONTRACT_VERSION: u32 = 1;

/// The wire convention, as published in `info.spec`.
pub const SPEC: &str = "mqtt-smarthome 2.0";

/// Milliseconds since the Unix epoch, the timestamp form of `{val, ts, lc}`.
pub fn to_ms(time: DateTime<Utc>) -> i64 {
    time.timestamp_millis()
}

/// The `{"val", "ts", "lc"}` object of every `status/*` value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatePayload {
    /// The value, a JSON scalar.
    pub val: serde_json::Value,
    /// When the value was obtained, ms since the epoch.
    pub ts: i64,
    /// When the value last changed as seen by VAG2MQTT, ms since the epoch. `lc <= ts`.
    pub lc: i64,
}

/// What `info` says about the running service.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceInfo {
    /// The service name, `vag2mqtt`.
    pub name: String,
    /// The service version.
    pub version: String,
    /// When the service started.
    pub started_at: DateTime<Utc>,
}

/// The retained `info` payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InfoPayload {
    /// Service name.
    pub name: String,
    /// Service version.
    pub version: String,
    /// The wire convention, [`SPEC`].
    pub spec: String,
    /// The contract version, [`CONTRACT_VERSION`].
    pub contract_version: u32,
    /// When the service started, RFC 3339.
    pub started_at: DateTime<Utc>,
}

impl InfoPayload {
    /// Builds the payload for a service.
    pub fn new(info: &ServiceInfo) -> Self {
        Self {
            name: info.name.clone(),
            version: info.version.clone(),
            spec: SPEC.to_string(),
            contract_version: CONTRACT_VERSION,
            started_at: info.started_at,
        }
    }
}

/// The identity a `full` payload carries next to the state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VehicleMeta {
    /// The VIN.
    pub vin: Vin,
    /// The brand.
    pub brand: Brand,
    /// The manufacturer's model designation.
    pub model: Option<String>,
    /// The user's display name.
    pub display_name: Option<String>,
}

/// The retained `status/<VIN>/full` payload: the lossless snapshot with identity and timestamps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FullPayload {
    /// [`CONTRACT_VERSION`].
    pub contract_version: u32,
    /// The VIN.
    pub vin: Vin,
    /// The brand.
    pub brand: Brand,
    /// The model designation, if known.
    pub model: Option<String>,
    /// The display name, if set.
    pub display_name: Option<String>,
    /// When the snapshot was fetched, RFC 3339.
    pub fetched_at: DateTime<Utc>,
    /// When this payload was published, RFC 3339.
    pub published_at: DateTime<Utc>,
    /// The snapshot in its domain serde form.
    pub state: VehicleState,
}

impl FullPayload {
    /// Builds the payload.
    pub fn new(meta: &VehicleMeta, state: &VehicleState, published_at: DateTime<Utc>) -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            vin: meta.vin.clone(),
            brand: meta.brand,
            model: meta.model.clone(),
            display_name: meta.display_name.clone(),
            fetched_at: state.fetched_at,
            published_at,
            state: state.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn state_payload_uses_integer_milliseconds() {
        let ts = Utc
            .with_ymd_and_hms(2026, 9, 22, 10, 0, 0)
            .single()
            .unwrap();
        let payload = StatePayload {
            val: serde_json::json!(80),
            ts: to_ms(ts),
            lc: to_ms(ts),
        };
        let json = serde_json::to_string(&payload).unwrap();
        let ms = ts.timestamp_millis();
        assert_eq!(
            ms.to_string().len(),
            13,
            "ms since epoch has 13 digits in 2026"
        );
        assert_eq!(json, format!(r#"{{"val":80,"ts":{ms},"lc":{ms}}}"#));
    }

    #[test]
    fn info_payload_carries_the_spec_fields() {
        let started_at = Utc
            .with_ymd_and_hms(2026, 9, 22, 10, 0, 0)
            .single()
            .unwrap();
        let info = InfoPayload::new(&ServiceInfo {
            name: "vag2mqtt".into(),
            version: "0.1.0".into(),
            started_at,
        });
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["name"], "vag2mqtt");
        assert_eq!(json["version"], "0.1.0");
        assert_eq!(json["spec"], "mqtt-smarthome 2.0");
        assert_eq!(json["contract_version"], 1);
        assert_eq!(json["started_at"], "2026-09-22T10:00:00Z");
    }
}
