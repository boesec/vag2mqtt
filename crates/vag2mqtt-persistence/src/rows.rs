//! Conversions between database rows and domain types.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use vag2mqtt_domain::{ErrorCategory, LastError};

use crate::error::PersistenceError;

/// Formats a timestamp the way every column stores it.
pub(crate) fn ts(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The current time, formatted for storage.
pub(crate) fn now() -> String {
    ts(Utc::now())
}

/// Parses a stored timestamp.
pub(crate) fn parse_ts(table: &'static str, text: &str) -> Result<DateTime<Utc>, PersistenceError> {
    DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .map_err(|e| PersistenceError::InvalidRow {
            table,
            reason: format!("timestamp: {e}"),
        })
}

/// Parses an optional stored timestamp.
pub(crate) fn parse_opt_ts(
    table: &'static str,
    text: Option<&str>,
) -> Result<Option<DateTime<Utc>>, PersistenceError> {
    text.map(|t| parse_ts(table, t)).transpose()
}

/// The serde string form of a unit enum variant.
pub(crate) fn enum_str<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        // Every enum stored through this helper serialises to a string; anything else is a bug
        // in this crate, and storing the JSON form keeps the row readable for diagnosis.
        Ok(other) => other.to_string(),
        Err(e) => e.to_string(),
    }
}

/// Parses the serde string form of a unit enum variant.
pub(crate) fn parse_enum<T: DeserializeOwned>(
    table: &'static str,
    column: &str,
    text: &str,
) -> Result<T, PersistenceError> {
    serde_json::from_value(serde_json::Value::String(text.to_string())).map_err(|_| {
        PersistenceError::InvalidRow {
            table,
            reason: format!("{column}: unknown value"),
        }
    })
}

/// Three nullable columns that together form an optional [`LastError`].
pub(crate) fn parse_last_error(
    table: &'static str,
    at: Option<&str>,
    category: Option<&str>,
    message: Option<&str>,
) -> Result<Option<LastError>, PersistenceError> {
    match (at, category, message) {
        (Some(at), Some(category), Some(message)) => Ok(Some(LastError {
            at: parse_ts(table, at)?,
            category: parse_enum::<ErrorCategory>(table, "last_error_category", category)?,
            message: message.to_string(),
        })),
        _ => Ok(None),
    }
}

/// The three columns for an optional [`LastError`].
pub(crate) fn last_error_columns(
    error: Option<&LastError>,
) -> (Option<String>, Option<String>, Option<String>) {
    match error {
        Some(e) => (
            Some(ts(e.at)),
            Some(enum_str(&e.category)),
            Some(e.message.clone()),
        ),
        None => (None, None, None),
    }
}
