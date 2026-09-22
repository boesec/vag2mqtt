//! Account: one user login to a manufacturer service (requirements 4.1).

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::LastError;
use crate::id::{AccountId, Brand};

/// An account as stored and shown, without its credentials or session.
///
/// Credentials and session material are connector-api types (WP-03) and are stored encrypted by
/// the persistence layer (WP-02); they are never part of this record.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// Internal identifier, also used in MQTT topics.
    pub id: AccountId,
    /// Which manufacturer service the account belongs to.
    pub brand: Brand,
    /// The login identifier, usually an email address. Never put it into a tracing span; use
    /// the `id`.
    pub username: String,
    /// Whether the runtime should poll this account at all.
    pub enabled: bool,
    /// How often the account's vehicles are polled.
    pub polling: PollingConfig,
    /// The current connection state as seen by the runtime.
    pub connection_state: AccountConnectionState,
    /// When the manufacturer service last answered successfully.
    pub last_success_at: Option<DateTime<Utc>>,
    /// The last error, if any, for diagnostics.
    pub last_error: Option<LastError>,
}

/// Polling configuration of an account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollingConfig {
    /// The interval between two polls of the same account.
    ///
    /// Serialised as whole seconds under `interval_secs`. A connector may declare a minimum that
    /// the runtime enforces (FR-013).
    #[serde(rename = "interval_secs", with = "crate::serde_helpers::duration_secs")]
    pub interval: Duration,
}

/// The connection state of an account (FR-018, account availability).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountConnectionState {
    /// Not attempted since the service started.
    Pending,
    /// Authenticated and delivering data.
    Ok,
    /// Credentials rejected, or two factor / CAPTCHA required. No automatic retry (ER-005).
    AuthError,
    /// The manufacturer signalled a rate limit; polling is paused (ER-004).
    RateLimited,
    /// The manufacturer service cannot be reached.
    Unreachable,
    /// Repeated failures of another kind (ER-003).
    Error,
    /// The account is disabled by the user.
    Disabled,
}

impl AccountConnectionState {
    /// Every variant, in declaration order.
    pub const ALL: &'static [AccountConnectionState] = &[
        AccountConnectionState::Pending,
        AccountConnectionState::Ok,
        AccountConnectionState::AuthError,
        AccountConnectionState::RateLimited,
        AccountConnectionState::Unreachable,
        AccountConnectionState::Error,
        AccountConnectionState::Disabled,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polling_interval_serialises_as_whole_seconds() {
        let polling = PollingConfig {
            interval: Duration::from_secs(300),
        };
        let json = serde_json::to_string(&polling).unwrap();
        assert_eq!(json, r#"{"interval_secs":300}"#);
        let back: PollingConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, polling);
    }

    #[test]
    fn connection_state_round_trips() {
        for state in AccountConnectionState::ALL {
            let json = serde_json::to_string(state).unwrap();
            let back: AccountConnectionState = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, state);
        }
        assert_eq!(
            serde_json::to_string(&AccountConnectionState::AuthError).unwrap(),
            r#""auth_error""#
        );
    }
}
