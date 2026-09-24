//! A 500 carries a correlation id that also appears in the log, and no internal detail.
//!
//! Own test binary, because it captures the `tracing` output and that subscriber is thread local.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::sync::{Arc, Mutex};

use axum::response::IntoResponse;
use tracing_subscriber::fmt::MakeWriter;
use vag2mqtt_admin::{ApiError, ErrorBody};
use vag2mqtt_persistence::PersistenceError;
use vag2mqtt_runtime::RuntimeError;

/// A path that must never reach a client.
const SECRET_DETAIL: &str = "C:/Users/someone/data/vag2mqtt.db";

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LogBuffer;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[tokio::test]
async fn the_reference_links_the_response_to_the_log_without_leaking() {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // A persistence failure carrying a path, which is exactly what must not be returned.
    let runtime_error = RuntimeError::Persistence(PersistenceError::InvalidRow {
        table: "accounts",
        reason: SECRET_DETAIL.to_string(),
    });
    let response = ApiError::from(runtime_error).into_response();
    assert_eq!(
        response.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );

    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let body: ErrorBody = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(body.error, "internal");
    assert!(
        !text.contains(SECRET_DETAIL) && !text.contains("accounts"),
        "the response leaked internal detail: {text}"
    );

    let reference = body.reference.expect("a 500 carries a reference");
    assert_eq!(reference.len(), 8);

    let log = String::from_utf8_lossy(&buffer.0.lock().unwrap()).into_owned();
    assert!(
        log.contains(&reference),
        "the reference {reference} is not in the log:\n{log}"
    );
    assert!(
        log.contains(SECRET_DETAIL),
        "the real cause belongs in the log:\n{log}"
    );
}
