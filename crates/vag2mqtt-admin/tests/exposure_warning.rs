//! Binding to a non loopback address without authentication logs exactly one warning.
//!
//! Own test binary: the captured `tracing` subscriber is thread local.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;
use vag2mqtt_admin::warn_if_exposed;

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

#[test]
fn a_wide_bind_warns_once_and_a_loopback_bind_stays_silent() {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    for quiet in ["127.0.0.1:8080", "[::1]:8080"] {
        warn_if_exposed(quiet.parse::<SocketAddr>().unwrap());
    }
    let after_loopback = String::from_utf8_lossy(&buffer.0.lock().unwrap()).into_owned();
    assert!(
        after_loopback.is_empty(),
        "a loopback bind must say nothing:\n{after_loopback}"
    );

    warn_if_exposed("0.0.0.0:8080".parse::<SocketAddr>().unwrap());
    let log = String::from_utf8_lossy(&buffer.0.lock().unwrap()).into_owned();
    assert_eq!(
        log.matches("no authentication").count(),
        1,
        "exactly one warning per bind:\n{log}"
    );
    assert!(log.contains("WARN"), "{log}");
    assert!(log.contains("0.0.0.0:8080"), "{log}");
}
