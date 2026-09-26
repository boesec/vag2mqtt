//! Connector errors and their classification.

use std::time::Duration;

use vag2mqtt_domain::ErrorCategory;

/// What went wrong in a connector call.
///
/// `Display` never includes a URL, a header, a body or a token. Connectors construct the
/// `detail` and `code` strings from sanitised material only.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ConnectorError {
    /// The manufacturer rejected the user name or password.
    #[error("the manufacturer rejected the credentials")]
    InvalidCredentials,
    /// The account requires a second factor the service cannot provide.
    #[error("the account requires two factor authentication, which is not supported")]
    TwoFactorRequired,
    /// The login flow presented a CAPTCHA.
    #[error("the login flow requires a CAPTCHA, which is not supported")]
    CaptchaRequired,
    /// The session could not be refreshed; a new login is needed.
    #[error("the session has expired")]
    SessionExpired,
    /// The manufacturer signalled throttling.
    #[error("the manufacturer is rate limiting requests")]
    RateLimited {
        /// How long the manufacturer asked us to wait, if it said.
        retry_after: Option<Duration>,
    },
    /// DNS, TCP, TLS or timeout trouble on the way to the manufacturer.
    #[error("network error: {detail}")]
    Network {
        /// A sanitised description such as `connection timed out` or `dns lookup failed`.
        detail: String,
    },
    /// The manufacturer answered with an error of its own.
    #[error("manufacturer error{}{}", fmt_status(.status), fmt_code(.code))]
    Manufacturer {
        /// The HTTP status, if the error came with one.
        status: Option<u16>,
        /// The manufacturer's error identifier, for example `login.error.throttled`.
        code: Option<String>,
    },
    /// The answer arrived but could not be understood.
    #[error("could not parse the manufacturer response ({context})")]
    Parsing {
        /// Which response or field, in a few words.
        context: &'static str,
    },
    /// The manufacturer does not know the VIN under this account.
    #[error("the vehicle is not known to the manufacturer under this account")]
    VehicleNotFound,
    /// The connector does not support this operation.
    #[error("operation not supported by this connector: {operation}")]
    Unsupported {
        /// The operation.
        operation: &'static str,
    },
}

fn fmt_status(status: &Option<u16>) -> String {
    status.map(|s| format!(" (HTTP {s})")).unwrap_or_default()
}

fn fmt_code(code: &Option<String>) -> String {
    code.as_ref().map(|c| format!(": {c}")).unwrap_or_default()
}

/// What the supervisor does about an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorClass {
    /// Credentials are wrong or the account needs human interaction. Stop until the user acts.
    AuthInvalid,
    /// Log in again with the stored password, then retry once.
    SessionExpired,
    /// Pause the whole account for a while.
    RateLimited,
    /// Retry with backoff.
    Transient,
    /// Record and move on; retry on the next regular tick.
    Permanent,
}

impl ConnectorError {
    /// A network error with a sanitised detail.
    pub fn network(detail: impl Into<String>) -> Self {
        ConnectorError::Network {
            detail: detail.into(),
        }
    }

    /// How the supervisor should react.
    pub fn class(&self) -> ErrorClass {
        match self {
            ConnectorError::InvalidCredentials
            | ConnectorError::TwoFactorRequired
            | ConnectorError::CaptchaRequired => ErrorClass::AuthInvalid,
            ConnectorError::SessionExpired => ErrorClass::SessionExpired,
            ConnectorError::RateLimited { .. } => ErrorClass::RateLimited,
            ConnectorError::Network { .. } => ErrorClass::Transient,
            ConnectorError::Manufacturer { status, .. } => match status {
                None => ErrorClass::Transient,
                Some(429) => ErrorClass::RateLimited,
                Some(401 | 403) => ErrorClass::SessionExpired,
                Some(s) if *s >= 500 => ErrorClass::Transient,
                Some(_) => ErrorClass::Permanent,
            },
            ConnectorError::Parsing { .. }
            | ConnectorError::VehicleNotFound
            | ConnectorError::Unsupported { .. } => ErrorClass::Permanent,
        }
    }

    /// What the user sees as the error's category.
    pub fn category(&self) -> ErrorCategory {
        match self {
            ConnectorError::InvalidCredentials
            | ConnectorError::TwoFactorRequired
            | ConnectorError::CaptchaRequired
            | ConnectorError::SessionExpired => ErrorCategory::Auth,
            ConnectorError::RateLimited { .. } => ErrorCategory::RateLimit,
            ConnectorError::Network { .. } => ErrorCategory::Network,
            ConnectorError::Manufacturer { status, .. } => match status {
                Some(429) => ErrorCategory::RateLimit,
                Some(401 | 403) => ErrorCategory::Auth,
                _ => ErrorCategory::Manufacturer,
            },
            ConnectorError::Parsing { .. } => ErrorCategory::Parsing,
            ConnectorError::VehicleNotFound => ErrorCategory::Manufacturer,
            ConnectorError::Unsupported { .. } => ErrorCategory::Configuration,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manufacturer(status: Option<u16>) -> ConnectorError {
        ConnectorError::Manufacturer { status, code: None }
    }

    #[test]
    fn classification_table() {
        let table: Vec<(ConnectorError, ErrorClass, ErrorCategory)> = vec![
            (
                ConnectorError::InvalidCredentials,
                ErrorClass::AuthInvalid,
                ErrorCategory::Auth,
            ),
            (
                ConnectorError::TwoFactorRequired,
                ErrorClass::AuthInvalid,
                ErrorCategory::Auth,
            ),
            (
                ConnectorError::CaptchaRequired,
                ErrorClass::AuthInvalid,
                ErrorCategory::Auth,
            ),
            (
                ConnectorError::SessionExpired,
                ErrorClass::SessionExpired,
                ErrorCategory::Auth,
            ),
            (
                ConnectorError::RateLimited { retry_after: None },
                ErrorClass::RateLimited,
                ErrorCategory::RateLimit,
            ),
            (
                ConnectorError::network("timeout"),
                ErrorClass::Transient,
                ErrorCategory::Network,
            ),
            (
                manufacturer(None),
                ErrorClass::Transient,
                ErrorCategory::Manufacturer,
            ),
            (
                manufacturer(Some(500)),
                ErrorClass::Transient,
                ErrorCategory::Manufacturer,
            ),
            (
                manufacturer(Some(503)),
                ErrorClass::Transient,
                ErrorCategory::Manufacturer,
            ),
            (
                manufacturer(Some(429)),
                ErrorClass::RateLimited,
                ErrorCategory::RateLimit,
            ),
            (
                manufacturer(Some(401)),
                ErrorClass::SessionExpired,
                ErrorCategory::Auth,
            ),
            (
                manufacturer(Some(403)),
                ErrorClass::SessionExpired,
                ErrorCategory::Auth,
            ),
            (
                manufacturer(Some(400)),
                ErrorClass::Permanent,
                ErrorCategory::Manufacturer,
            ),
            (
                manufacturer(Some(404)),
                ErrorClass::Permanent,
                ErrorCategory::Manufacturer,
            ),
            (
                ConnectorError::Parsing { context: "x" },
                ErrorClass::Permanent,
                ErrorCategory::Parsing,
            ),
            (
                ConnectorError::VehicleNotFound,
                ErrorClass::Permanent,
                ErrorCategory::Manufacturer,
            ),
            (
                ConnectorError::Unsupported {
                    operation: "commands",
                },
                ErrorClass::Permanent,
                ErrorCategory::Configuration,
            ),
        ];
        for (error, class, category) in table {
            assert_eq!(error.class(), class, "{error}");
            assert_eq!(error.category(), category, "{error}");
        }
    }

    #[test]
    fn display_carries_only_sanitised_detail() {
        let error = ConnectorError::Manufacturer {
            status: Some(500),
            code: Some("login.error.throttled".into()),
        };
        assert_eq!(
            error.to_string(),
            "manufacturer error (HTTP 500): login.error.throttled"
        );
        assert_eq!(manufacturer(None).to_string(), "manufacturer error");
        assert_eq!(
            ConnectorError::network("connection timed out").to_string(),
            "network error: connection timed out"
        );
    }
}
