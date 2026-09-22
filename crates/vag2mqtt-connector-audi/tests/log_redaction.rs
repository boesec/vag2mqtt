//! At log level `trace`, a full login logs no token, no cookie and no password.
//!
//! Own test binary: the captured `tracing` subscriber is thread local.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::sync::{Arc, Mutex};

use tracing_subscriber::fmt::MakeWriter;
use url::Url;
use vag2mqtt_connector_api::{Connector, Credentials};
use vag2mqtt_connector_audi::auth::form::{Endpoints, FormLoginStrategy};
use vag2mqtt_connector_audi::{AudiConnector, TraceConfig};
use vag2mqtt_domain::Secret;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

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

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/audi/auth")
            .join(name),
    )
    .unwrap()
}

#[tokio::test]
async fn trace_level_logging_leaks_nothing() {
    let buffer = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(buffer.clone())
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let server = MockServer::start().await;
    let client = "09b6cbec-cd19-4589-82fd-363dfa8c24da@apps_vw-dilab_com";
    Mock::given(method("GET"))
        .and(path("/oidc/v1/authorize"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", "/signin")
                .insert_header("Set-Cookie", "SESSION=COOKIE-MARKER; Path=/"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signin"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("signin_email_form.html")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/signin-service/v1/{client}/login/identifier"
        )))
        .respond_with(ResponseTemplate::new(303).insert_header("Location", "/password"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/password"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(fixture("signin_password_form.html")),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/signin-service/v1/{client}/login/authenticate"
        )))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            "myaudi:///#state=abc&code=CODE-MARKER&id_token=ID-MARKER",
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/auth/v1/idk/oidc/token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"{"access_token":"ACCESS-MARKER","refresh_token":"REFRESH-MARKER","expires_in":3600}"#,
        ))
        .mount(&server)
        .await;

    let base = Url::parse(&server.uri()).unwrap();
    let endpoints = Endpoints {
        identity_base: base.clone(),
        bff_base: base,
        ..Endpoints::default()
    };
    let connector = AudiConnector::with_strategy(
        Arc::new(FormLoginStrategy::with_endpoints(endpoints)),
        TraceConfig::disabled(),
    )
    .unwrap();
    let credentials = Credentials {
        username: "driver@example.test".into(),
        password: Secret::new("hunter2-MARKER".into()),
    };
    connector.login(&credentials).await.unwrap();

    let log = String::from_utf8_lossy(&buffer.0.lock().unwrap()).into_owned();
    assert!(log.contains("login succeeded"), "{log}");
    for marker in [
        "hunter2-MARKER",
        "COOKIE-MARKER",
        "CODE-MARKER",
        "ID-MARKER",
        "ACCESS-MARKER",
        "REFRESH-MARKER",
    ] {
        assert!(
            !log.contains(marker),
            "{marker} leaked into the log:\n{log}"
        );
    }
}
