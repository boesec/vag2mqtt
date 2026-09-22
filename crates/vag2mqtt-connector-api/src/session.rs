//! Session state: what a connector needs between calls.

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use vag2mqtt_domain::Brand;

use crate::error::ConnectorError;

/// Everything a connector needs between calls for one account: tokens, expiry times, whatever
/// the brand requires. Owned by the runtime's account task and stored encrypted by persistence.
///
/// `payload` is connector private. The runtime and persistence never look inside; the brand
/// crate converts it to and from its own typed session.
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionState {
    /// Which connector produced this session.
    pub brand: Brand,
    /// Connector private content. Treated as secret.
    pub payload: serde_json::Value,
    /// The earliest moment a refresh is needed, if the connector knows it. The runtime uses it
    /// to refresh before a call rather than after a failure.
    pub expires_at: Option<DateTime<Utc>>,
}

impl SessionState {
    /// A session with no expiry information.
    pub fn new(brand: Brand, payload: serde_json::Value) -> Self {
        Self {
            brand,
            payload,
            expires_at: None,
        }
    }

    /// `true` if the session has an expiry and it lies at or before `now`.
    pub fn is_expired_at(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_some_and(|expires| expires <= now)
    }

    /// The bytes persistence stores, encrypted.
    pub fn to_bytes(&self) -> Result<Vec<u8>, ConnectorError> {
        serde_json::to_vec(self).map_err(|_| ConnectorError::Parsing {
            context: "session serialisation",
        })
    }

    /// Parses stored bytes. Garbage yields a [`ConnectorError::Parsing`] error, never a panic.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ConnectorError> {
        serde_json::from_slice(bytes).map_err(|_| ConnectorError::Parsing {
            context: "stored session",
        })
    }
}

impl fmt::Debug for SessionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionState")
            .field("brand", &self.brand)
            .field("expires_at", &self.expires_at)
            .field("payload", &"[REDACTED]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    const TOKEN: &str = "eyJ-SECRET-TOKEN-MARKER";

    fn session() -> SessionState {
        SessionState {
            brand: Brand::Audi,
            payload: serde_json::json!({ "access_token": TOKEN, "url": "https://example.test/x" }),
            expires_at: Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).single(),
        }
    }

    #[test]
    fn debug_redacts_the_payload() {
        let debug = format!("{:?}", session());
        assert!(debug.contains("Audi"));
        assert!(!debug.contains(TOKEN));
        assert!(!debug.contains("https://"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn bytes_round_trip_and_garbage_is_an_error() {
        let original = session();
        let bytes = original.to_bytes().unwrap();
        let back = SessionState::from_bytes(&bytes).unwrap();
        assert_eq!(back.brand, original.brand);
        assert_eq!(back.payload, original.payload);
        assert_eq!(back.expires_at, original.expires_at);

        let garbage = SessionState::from_bytes(b"\xff\x00not json");
        assert!(matches!(garbage, Err(ConnectorError::Parsing { .. })));
    }

    #[test]
    fn expiry_comparison() {
        let session = session();
        let before = Utc
            .with_ymd_and_hms(2026, 9, 22, 11, 59, 59)
            .single()
            .unwrap();
        let at = Utc
            .with_ymd_and_hms(2026, 9, 22, 12, 0, 0)
            .single()
            .unwrap();
        assert!(!session.is_expired_at(before));
        assert!(session.is_expired_at(at));
        assert!(!SessionState::new(Brand::Audi, serde_json::json!({})).is_expired_at(at));
    }
}
