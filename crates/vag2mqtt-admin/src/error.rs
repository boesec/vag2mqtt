//! The API's error shape.
//!
//! Slim and project specific, chosen over RFC 9457 on 2026-09-22:
//!
//! ```json
//! { "error": "validation", "message": "...", "field": "polling_interval" }
//! ```

use axum::Json;
use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use vag2mqtt_runtime::RuntimeError;

const LOG: &str = "vag2mqtt::admin";

/// The body of every error response.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    /// A stable, machine readable slug.
    pub error: String,
    /// A human readable message. Never carries internal detail.
    pub message: String,
    /// The input field at fault, when the problem can be attributed to one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// A correlation id repeated in the log, present only on `internal`, so a user can quote it
    /// without the API leaking a database path or a connection string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
}

/// Everything a handler can fail with.
#[derive(Debug)]
pub enum ApiError {
    /// Input that does not pass validation. 422.
    Validation {
        /// What is wrong, for a human.
        message: String,
        /// The field at fault, when there is one.
        field: Option<&'static str>,
    },
    /// A malformed request: bad JSON, a bad path parameter. 400.
    BadRequest(String),
    /// The entity does not exist. 404.
    NotFound(&'static str),
    /// The runtime is not answering. 503.
    Unavailable,
    /// The MQTT layer failed. 502.
    Mqtt(String),
    /// Anything we do not explain. 500, with a correlation id.
    Internal(String),
}

impl ApiError {
    /// A validation failure attributed to a field.
    pub fn field(field: &'static str, message: impl Into<String>) -> Self {
        ApiError::Validation {
            message: message.into(),
            field: Some(field),
        }
    }

    /// The status and body this error becomes.
    fn parts(self) -> (StatusCode, ErrorBody) {
        match self {
            ApiError::Validation { message, field } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorBody {
                    error: "validation".into(),
                    message,
                    field: field.map(str::to_string),
                    reference: None,
                },
            ),
            ApiError::BadRequest(message) => (
                StatusCode::BAD_REQUEST,
                ErrorBody {
                    error: "bad_request".into(),
                    message,
                    field: None,
                    reference: None,
                },
            ),
            ApiError::NotFound(entity) => (
                StatusCode::NOT_FOUND,
                ErrorBody {
                    error: "not_found".into(),
                    message: format!("{entity} not found"),
                    field: None,
                    reference: None,
                },
            ),
            ApiError::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorBody {
                    error: "unavailable".into(),
                    message: "the runtime is not answering; try again shortly".into(),
                    field: None,
                    reference: None,
                },
            ),
            ApiError::Mqtt(detail) => (
                StatusCode::BAD_GATEWAY,
                ErrorBody {
                    error: "mqtt".into(),
                    message: format!("the MQTT layer refused the request: {detail}"),
                    field: None,
                    reference: None,
                },
            ),
            ApiError::Internal(detail) => {
                let reference = short_reference();
                // The detail goes to the log, never to the client.
                tracing::error!(target: LOG, reference = %reference, detail = %detail, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    ErrorBody {
                        error: "internal".into(),
                        message: "something went wrong inside the service".into(),
                        field: None,
                        reference: Some(reference),
                    },
                )
            }
        }
    }
}

/// Eight hex characters: long enough to find a line in a log, short enough to read out.
fn short_reference() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect()
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = self.parts();
        (status, Json(body)).into_response()
    }
}

impl From<RuntimeError> for ApiError {
    fn from(error: RuntimeError) -> Self {
        match error {
            RuntimeError::Invalid { reason } => ApiError::Validation {
                message: reason,
                field: None,
            },
            RuntimeError::IntervalBelowMinimum { minimum } => ApiError::field(
                "polling_interval",
                format!(
                    "polling interval below the minimum of {} seconds",
                    minimum.as_secs()
                ),
            ),
            RuntimeError::UnknownBrand(brand) => {
                ApiError::field("brand", format!("no connector for brand {brand}"))
            }
            RuntimeError::NotFound { entity } => ApiError::NotFound(entity),
            RuntimeError::Stopped => ApiError::Unavailable,
            RuntimeError::Mqtt(error) => ApiError::Mqtt(error.to_string()),
            RuntimeError::Persistence(error) => ApiError::Internal(error.to_string()),
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        ApiError::BadRequest(rejection.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        ApiError::BadRequest(rejection.body_text())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn status_and_body(error: ApiError) -> (StatusCode, ErrorBody) {
        error.parts()
    }

    #[test]
    fn the_mapping_table_holds() {
        let rows: Vec<(RuntimeError, StatusCode, &str, Option<&str>)> = vec![
            (
                RuntimeError::Invalid {
                    reason: "username is empty".into(),
                },
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                None,
            ),
            (
                RuntimeError::IntervalBelowMinimum {
                    minimum: Duration::from_secs(300),
                },
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                Some("polling_interval"),
            ),
            (
                RuntimeError::UnknownBrand(vag2mqtt_domain::Brand::Cupra),
                StatusCode::UNPROCESSABLE_ENTITY,
                "validation",
                Some("brand"),
            ),
            (
                RuntimeError::NotFound { entity: "account" },
                StatusCode::NOT_FOUND,
                "not_found",
                None,
            ),
            (
                RuntimeError::Stopped,
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                None,
            ),
            (
                RuntimeError::Mqtt(vag2mqtt_mqtt::MqttError::Backpressure),
                StatusCode::BAD_GATEWAY,
                "mqtt",
                None,
            ),
        ];
        for (runtime_error, status, slug, field) in rows {
            let rendered = format!("{runtime_error}");
            let (actual_status, body) = status_and_body(ApiError::from(runtime_error));
            assert_eq!(actual_status, status, "{rendered}");
            assert_eq!(body.error, slug, "{rendered}");
            assert_eq!(body.field.as_deref(), field, "{rendered}");
            assert!(body.reference.is_none(), "{rendered}");
        }
    }

    #[test]
    fn an_internal_error_hides_its_detail_and_carries_a_reference() {
        let error = RuntimeError::Persistence(vag2mqtt_persistence::PersistenceError::NotFound {
            entity: "C:/secret/path/vag2mqtt.db",
        });
        let (status, body) = status_and_body(ApiError::from(error));
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body.error, "internal");
        assert!(!body.message.contains("secret"), "{}", body.message);
        assert!(!body.message.contains("path"), "{}", body.message);
        let reference = body.reference.expect("internal errors carry a reference");
        assert_eq!(reference.len(), 8);
        assert!(reference.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn the_field_is_omitted_when_absent() {
        let (_, body) = status_and_body(ApiError::NotFound("vehicle"));
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("field").is_none());
        assert!(json.get("reference").is_none());
        assert_eq!(json["error"], "not_found");
        assert_eq!(json["message"], "vehicle not found");
    }
}
