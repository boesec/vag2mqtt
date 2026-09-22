//! Error categories for diagnostics and the validation errors of this crate.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Coarse classification of an error for diagnostics (FR-021).
///
/// This is the *what*, shown to the user next to the message. Whether an error is worth a
/// retry is a separate classification that lives with the connector errors (WP-03).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// Credentials rejected, token exchange failed, two factor or CAPTCHA required.
    Auth,
    /// DNS, TCP, TLS or timeout problems on the way to the manufacturer.
    Network,
    /// The manufacturer signalled a rate limit or throttling.
    RateLimit,
    /// The manufacturer answered, but with an error of its own (HTTP 5xx, error body).
    Manufacturer,
    /// The answer arrived but could not be understood.
    Parsing,
    /// Reading from or writing to the local database failed.
    Persistence,
    /// The stored configuration is unusable (for example an interval below the connector minimum).
    Configuration,
    /// A bug or an invariant violation inside VAG2MQTT.
    Internal,
}

impl ErrorCategory {
    /// Every variant, in declaration order. Used by tests and by documentation generators.
    pub const ALL: &'static [ErrorCategory] = &[
        ErrorCategory::Auth,
        ErrorCategory::Network,
        ErrorCategory::RateLimit,
        ErrorCategory::Manufacturer,
        ErrorCategory::Parsing,
        ErrorCategory::Persistence,
        ErrorCategory::Configuration,
        ErrorCategory::Internal,
    ];
}

/// The last error recorded for an account or a vehicle (FR-021).
///
/// `message` is user facing. The layer that creates a `LastError` is responsible for a message
/// free of secrets; the domain does not scan strings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastError {
    /// When the error occurred.
    pub at: DateTime<Utc>,
    /// What kind of error it was.
    pub category: ErrorCategory,
    /// A short, secret free description for the UI and the API.
    pub message: String,
}

/// Validation errors raised by the constructors in this crate.
///
/// The messages deliberately carry no user data: a rejected VIN or coordinate must not end up in
/// a log line through an error message.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    /// The string is not a well formed VIN.
    #[error("invalid VIN: {reason}")]
    InvalidVin {
        /// Why the VIN was rejected.
        reason: &'static str,
    },
    /// The string is not usable as an account identifier.
    #[error("invalid account id: {reason}")]
    InvalidAccountId {
        /// Why the identifier was rejected.
        reason: &'static str,
    },
    /// A percentage above 100.
    #[error("percent value {value} is above 100")]
    PercentOutOfRange {
        /// The rejected value.
        value: u8,
    },
    /// A latitude, longitude or heading outside its valid range.
    #[error("{which} is out of range")]
    CoordinateOutOfRange {
        /// Which coordinate was rejected (`latitude`, `longitude` or `heading`).
        which: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_carry_no_user_data() {
        let vin = DomainError::InvalidVin {
            reason: "wrong length",
        };
        assert_eq!(vin.to_string(), "invalid VIN: wrong length");
        let coordinate = DomainError::CoordinateOutOfRange { which: "latitude" };
        assert!(!coordinate.to_string().chars().any(|c| c.is_ascii_digit()));
    }
}
