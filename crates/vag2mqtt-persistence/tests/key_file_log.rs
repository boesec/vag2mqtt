//! The master key and the account password never reach the log.
//!
//! Lives in its own test binary: the captured `tracing` subscriber is thread local, and other
//! tests in the same process would race on the callsite interest cache.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tracing_subscriber::fmt::MakeWriter;
use vag2mqtt_domain::{Brand, PollingConfig, Secret};
use vag2mqtt_persistence::{Database, MasterKeySource, NewAccount};

const PASSWORD_MARKER: &str = "PLAINTEXT-PASSWORD-MARKER-7f3a";

/// Collects tracing output so a test can scan it.
#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl LogBuffer {
    fn contents(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

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
async fn key_material_never_reaches_the_log() {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path(), MasterKeySource::KeyFile)
        .await
        .unwrap();
    db.accounts()
        .insert(
            NewAccount {
                brand: Brand::Audi,
                username: "log@example.test".into(),
                enabled: true,
                polling: PollingConfig {
                    interval: Duration::from_secs(300),
                },
            },
            &Secret::new(PASSWORD_MARKER.into()),
        )
        .await
        .unwrap();
    db.close().await;

    let key_hex = std::fs::read_to_string(dir.path().join("master.key")).unwrap();
    let key_hex = key_hex.trim();
    assert_eq!(key_hex.len(), 64);
    let log = buffer.contents();
    assert!(
        log.contains("master key file"),
        "expected a log line about the key file:\n{log}"
    );
    assert!(!log.contains(key_hex), "the key leaked into the log");
    assert!(
        !log.contains(&key_hex[..16]),
        "a key prefix leaked into the log"
    );
    assert!(
        !log.contains(PASSWORD_MARKER),
        "the password leaked into the log"
    );
}
