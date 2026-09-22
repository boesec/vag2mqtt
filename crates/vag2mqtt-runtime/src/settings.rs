//! Runtime settings with their defaults, stored as one JSON value in the `settings` table.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use vag2mqtt_persistence::Database;

use crate::error::RuntimeError;

/// The key under which the settings live in the database.
pub(crate) const SETTINGS_KEY: &str = "runtime";

/// Tunables of the supervisor and the account tasks. Every field has a default, so a partial
/// JSON value from the database is fine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RuntimeSettings {
    /// Polling interval for new accounts.
    #[serde(with = "secs")]
    pub default_polling_interval: Duration,
    /// First backoff after a transient failure.
    #[serde(with = "secs")]
    pub backoff_initial: Duration,
    /// Longest backoff.
    #[serde(with = "secs")]
    pub backoff_max: Duration,
    /// Multiplier per consecutive failure.
    pub backoff_factor: f64,
    /// Jitter as a fraction of the delay, applied symmetrically (0.2 means ±20 %).
    pub backoff_jitter: f64,
    /// Consecutive transient failures before an account or vehicle shows `error`.
    pub failures_to_error: u32,
    /// Pause after a rate limit without a `retry_after`.
    #[serde(with = "secs")]
    pub rate_limit_pause: Duration,
    /// A vehicle is stale when its data is older than this multiple of the polling interval.
    pub stale_multiplier: u32,
    /// How often vehicles are rediscovered.
    #[serde(with = "secs")]
    pub discovery_interval: Duration,
    /// Delay before an account task that panicked is spawned again.
    #[serde(with = "secs")]
    pub respawn_delay: Duration,
    /// Respawns allowed per hour per account before it stays down.
    pub respawns_per_hour: u32,
    /// How often the supervisor checks for stale vehicles.
    #[serde(with = "secs")]
    pub stale_check_interval: Duration,
}

impl Default for RuntimeSettings {
    fn default() -> Self {
        Self {
            default_polling_interval: Duration::from_secs(300),
            backoff_initial: Duration::from_secs(30),
            backoff_max: Duration::from_secs(30 * 60),
            backoff_factor: 2.0,
            backoff_jitter: 0.2,
            failures_to_error: 5,
            rate_limit_pause: Duration::from_secs(900),
            stale_multiplier: 3,
            discovery_interval: Duration::from_secs(24 * 3600),
            respawn_delay: Duration::from_secs(60),
            respawns_per_hour: 5,
            stale_check_interval: Duration::from_secs(60),
        }
    }
}

impl RuntimeSettings {
    /// Loads the settings, falling back to the defaults for missing values.
    pub async fn load(db: &Database) -> Result<Self, RuntimeError> {
        Ok(db
            .settings()
            .get::<RuntimeSettings>(SETTINGS_KEY)
            .await?
            .unwrap_or_default())
    }

    /// Stores the settings.
    pub async fn store(&self, db: &Database) -> Result<(), RuntimeError> {
        db.settings().set(SETTINGS_KEY, self).await?;
        Ok(())
    }
}

mod secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(super) fn serialize<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
        d.as_secs_f64().serialize(s)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = f64::deserialize(d)?;
        if !secs.is_finite() || secs < 0.0 {
            return Err(serde::de::Error::custom(
                "duration must be a non-negative number",
            ));
        }
        Ok(Duration::from_secs_f64(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_json_falls_back_to_defaults() {
        let settings: RuntimeSettings =
            serde_json::from_str(r#"{"stale_multiplier": 5, "backoff_initial": 10}"#).unwrap();
        assert_eq!(settings.stale_multiplier, 5);
        assert_eq!(settings.backoff_initial, Duration::from_secs(10));
        assert_eq!(settings.failures_to_error, 5);
        let json = serde_json::to_value(&settings).unwrap();
        assert_eq!(json["rate_limit_pause"], 900.0);
    }
}
