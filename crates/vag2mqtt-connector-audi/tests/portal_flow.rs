//! The EU Data Act portal login against a mock identity service and a mock portal.
//!
//! Two servers, because the route has to tell them apart: consent pages are only accepted while
//! the chase is still at the identity service, and arriving at the portal ends the chase.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use url::Url;
use vag2mqtt_connector_api::{Connector, ConnectorError, Credentials, ErrorClass};
use vag2mqtt_connector_audi::auth::portal::{
    AUDI_PORTAL_CLIENT_ID, EuDataActStrategy, portal_endpoints,
};
use vag2mqtt_connector_audi::{AudiConnector, AudiTokens, TraceConfig};
use vag2mqtt_domain::{DataSourceKind, Drivetrain, Secret, Vin};
use wiremock::matchers::{
    body_string_contains, header_regex, method, path, query_param, query_param_is_missing,
};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The client id the sign-in fixtures were recorded with; it only appears in their form actions.
const FIXTURE_CLIENT: &str = "09b6cbec-cd19-4589-82fd-363dfa8c24da@apps_vw-dilab_com";
const SESSION_COOKIE: &str = "SESSION-COOKIE-MARKER";
const VIN: &str = "WAUZZZ0000000TEST";

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/audi/auth")
        .join(name);
    std::fs::read_to_string(path).unwrap()
}

fn credentials() -> Credentials {
    Credentials {
        username: "driver@example.test".into(),
        password: Secret::new("hunter2-MARKER".into()),
    }
}

struct Servers {
    identity: MockServer,
    portal: MockServer,
}

impl Servers {
    async fn start() -> Self {
        let servers = Self {
            identity: MockServer::start().await,
            portal: MockServer::start().await,
        };
        servers.mount_identity().await;
        servers.mount_portal_login().await;
        servers
    }

    fn connector(&self, trace: TraceConfig) -> AudiConnector {
        let base = Url::parse(&self.portal.uri()).unwrap();
        let mut endpoints = portal_endpoints(&base);
        endpoints.identity_base = Url::parse(&self.identity.uri()).unwrap();
        AudiConnector::with_strategy(
            Arc::new(EuDataActStrategy::with_endpoints(base, endpoints)),
            trace,
        )
        .unwrap()
    }

    /// The identity service: authorize, both forms, and a password post that hands over to the
    /// portal's redirect URI with a code the portal, not we, exchanges.
    async fn mount_identity(&self) {
        let server = &self.identity;
        let redirect_uri = format!("{}/login", self.portal.uri());
        Mock::given(method("GET"))
            .and(path("/oidc/v1/authorize"))
            .and(query_param("client_id", AUDI_PORTAL_CLIENT_ID))
            .and(query_param("response_type", "code"))
            .and(query_param("scope", "openid cars profile"))
            .and(query_param("redirect_uri", redirect_uri.as_str()))
            .and(query_param("state", "de__en__AUDI"))
            // The portal exchanges the code without our verifier, so a PKCE challenge would
            // break its exchange (first live test, 2026-09-24).
            .and(query_param_is_missing("code_challenge"))
            .and(query_param_is_missing("code_challenge_method"))
            .and(query_param_is_missing("ui_locales"))
            .respond_with(ResponseTemplate::new(302).insert_header(
                "Location",
                format!("/signin-service/v1/signin/{FIXTURE_CLIENT}@relayState=relay-fixture"),
            ))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/signin-service/v1/signin/{FIXTURE_CLIENT}@relayState=relay-fixture"
            )))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(fixture("signin_email_form.html")),
            )
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/signin-service/v1/{FIXTURE_CLIENT}/login/identifier"
            )))
            .and(body_string_contains("email=driver%40example.test"))
            .respond_with(ResponseTemplate::new(303).insert_header(
                "Location",
                format!(
                    "/signin-service/v1/{FIXTURE_CLIENT}/login/authenticate?relayState=relay-fixture"
                ),
            ))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path(format!(
                "/signin-service/v1/{FIXTURE_CLIENT}/login/authenticate"
            )))
            .respond_with(
                ResponseTemplate::new(200).set_body_string(fixture("signin_password_form.html")),
            )
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path(format!(
                "/signin-service/v1/{FIXTURE_CLIENT}/login/authenticate"
            )))
            .and(body_string_contains("password=hunter2-MARKER"))
            .respond_with(
                ResponseTemplate::new(302).insert_header("Location", "/oidc/v1/oauth/sso"),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/oidc/v1/oauth/sso"))
            .respond_with(ResponseTemplate::new(302).insert_header(
                "Location",
                format!("{redirect_uri}?code=CODE-MARKER&state=s"),
            ))
            .mount(server)
            .await;
    }

    /// The portal: a priming cookie on its landing page, the session cookie on `/login`, then
    /// its application page.
    async fn mount_portal_login(&self) {
        let server = &self.portal;
        Mock::given(method("GET"))
            .and(path("/"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("Set-Cookie", "PRIME=prime; Path=/")
                    .set_body_string("<html>portal</html>"),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/login"))
            .and(query_param("code", "CODE-MARKER"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "/app")
                    .insert_header(
                        "Set-Cookie",
                        format!("SESSION={SESSION_COOKIE}; Path=/; HttpOnly").as_str(),
                    ),
            )
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/app"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>app</html>"))
            .mount(server)
            .await;
    }

    /// The vehicle list, answered only to a request carrying the session cookie.
    async fn mount_vehicles(&self, template: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path("/proxy_api/consent/me/vehicles"))
            .and(header_regex("cookie", &format!("SESSION={SESSION_COOKIE}")))
            .respond_with(template)
            .mount(&self.portal)
            .await;
    }
}

fn vehicle_list() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_string(format!(
        r#"[{{"vin":"{VIN}","modelName":"A6 e-tron","fuelType":"ELECTRIC","surprise":1}}]"#
    ))
}

#[tokio::test]
async fn the_default_connector_is_the_read_only_portal() {
    let connector = AudiConnector::new(TraceConfig::disabled()).unwrap();
    let info = connector.info();
    assert_eq!(connector.strategy_name(), "eu_data_act");
    assert_eq!(info.kind, DataSourceKind::Export);
    assert!(!info.supports_commands);
    assert_eq!(info.min_polling_interval.as_secs(), 15 * 60);
}

#[tokio::test]
async fn a_portal_login_yields_a_cookie_session_and_no_token() {
    let servers = Servers::start().await;
    servers.mount_vehicles(vehicle_list()).await;
    let connector = servers.connector(TraceConfig::disabled());

    let session = connector.login(&credentials()).await.unwrap();

    assert_eq!(session.payload["route"], "eu_data_act");
    assert!(session.payload.get("access_token").is_none());
    assert!(session.expires_at.is_none(), "the portal states no expiry");
    let stored = session.payload.to_string();
    assert!(stored.contains(SESSION_COOKIE), "the session is the cookie");
    assert!(
        !stored.contains("CODE-MARKER"),
        "the code was the portal's to use"
    );
    assert!(!stored.contains("hunter2"));
    assert!(!format!("{session:?}").contains(SESSION_COOKIE));
    assert!(AudiTokens::from_session(&session).is_err());
}

#[tokio::test]
async fn the_session_survives_persistence_and_lists_the_vehicles() {
    let servers = Servers::start().await;
    servers.mount_vehicles(vehicle_list()).await;
    let connector = servers.connector(TraceConfig::disabled());
    let session = connector.login(&credentials()).await.unwrap();

    // What persistence stores and loads back: the bytes, not the object.
    let bytes = session.to_bytes().unwrap();
    let mut restored = vag2mqtt_connector_api::SessionState::from_bytes(&bytes).unwrap();

    let vehicles = connector.list_vehicles(&mut restored).await.unwrap();
    assert_eq!(vehicles.len(), 1);
    assert_eq!(vehicles[0].vin, Vin::new(VIN).unwrap());
    assert_eq!(vehicles[0].model.as_deref(), Some("A6 e-tron"));
    assert_eq!(vehicles[0].drivetrain, Drivetrain::Electric);

    // Asking again is the refresh: the portal still accepts the cookie.
    connector.refresh(&mut restored).await.unwrap();
}

#[tokio::test]
async fn a_rejected_cookie_is_an_expired_session() {
    let servers = Servers::start().await;
    servers.mount_vehicles(vehicle_list()).await;
    let connector = servers.connector(TraceConfig::disabled());
    let mut session = connector.login(&credentials()).await.unwrap();

    // The portal forgets the session: every further request is answered 401.
    servers.portal.reset().await;
    Mock::given(method("GET"))
        .and(path("/proxy_api/consent/me/vehicles"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&servers.portal)
        .await;

    let error = connector.list_vehicles(&mut session).await.unwrap_err();
    assert!(matches!(error, ConnectorError::SessionExpired), "{error:?}");
    assert_eq!(error.class(), ErrorClass::SessionExpired);
    let error = connector.refresh(&mut session).await.unwrap_err();
    assert!(matches!(error, ConnectorError::SessionExpired), "{error:?}");
}

#[tokio::test]
async fn a_portal_without_the_browser_setup_names_the_setup_not_the_password() {
    let servers = Servers::start().await;
    servers.mount_vehicles(ResponseTemplate::new(403)).await;
    let connector = servers.connector(TraceConfig::disabled());

    let error = connector.login(&credentials()).await.unwrap_err();
    let ConnectorError::Manufacturer { status, code } = &error else {
        panic!("expected a manufacturer error, got {error:?}");
    };
    assert_eq!(*status, Some(403));
    let message = code.as_deref().unwrap_or_default();
    assert!(message.contains("consent"), "{message}");
    assert!(message.contains("continuous data request"), "{message}");
    assert_ne!(
        error.class(),
        ErrorClass::AuthInvalid,
        "the password is fine; stopping the account would be wrong"
    );
}

#[tokio::test]
async fn wrong_credentials_are_still_recognised_on_the_portal_route() {
    let identity = MockServer::start().await;
    let portal = MockServer::start().await;
    let servers = Servers { identity, portal };
    // The identifier post answers with the identity service's own error.
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&servers.portal)
        .await;
    Mock::given(method("GET"))
        .and(path("/oidc/v1/authorize"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("signin_email_form.html")))
        .mount(&servers.identity)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/signin-service/v1/{FIXTURE_CLIENT}/login/identifier"
        )))
        .respond_with(ResponseTemplate::new(303).insert_header(
            "Location",
            "/signin-service/v1/error?error=login.errors.password_invalid",
        ))
        .mount(&servers.identity)
        .await;
    let connector = servers.connector(TraceConfig::disabled());

    let error = connector.login(&credentials()).await.unwrap_err();
    assert!(
        matches!(error, ConnectorError::InvalidCredentials),
        "{error:?}"
    );
}

#[tokio::test]
async fn trace_files_of_a_portal_login_leak_nothing() {
    let servers = Servers::start().await;
    servers.mount_vehicles(vehicle_list()).await;
    let directory = tempfile::tempdir().unwrap();
    let connector = servers.connector(TraceConfig::into_directory(directory.path()));

    connector.login(&credentials()).await.unwrap();

    let mut files = Vec::new();
    let mut pending = vec![directory.path().to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    assert!(files.len() > 5, "a trace was written: {files:?}");
    let summary = files
        .iter()
        .find(|p| p.ends_with("summary.txt"))
        .map(|p| std::fs::read_to_string(p).unwrap())
        .expect("a summary");
    assert!(summary.contains("portal_landing"), "{summary}");
    assert!(summary.contains("1 vehicle(s)"), "{summary}");
    for file in &files {
        let text = std::fs::read_to_string(file).unwrap();
        for secret in [
            SESSION_COOKIE,
            "prime",
            "hunter2",
            "CODE-MARKER",
            VIN,
            "driver@example",
        ] {
            assert!(
                !text.contains(secret),
                "{} contains {secret}",
                file.display()
            );
        }
    }
}

#[tokio::test]
async fn a_login_that_ends_on_a_portal_error_page_is_not_a_session() {
    let servers = Servers::start().await;
    servers.mount_vehicles(vehicle_list()).await;
    // The portal fails its own code exchange and sends the browser to an error page.
    servers.portal.reset().await;
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&servers.portal)
        .await;
    Mock::given(method("GET"))
        .and(path("/login"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/error?reason=x"))
        .mount(&servers.portal)
        .await;
    Mock::given(method("GET"))
        .and(path("/error"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>error</html>"))
        .mount(&servers.portal)
        .await;
    let connector = servers.connector(TraceConfig::disabled());

    let error = connector.login(&credentials()).await.unwrap_err();
    let ConnectorError::Manufacturer { code, .. } = &error else {
        panic!("expected a manufacturer error, got {error:?}");
    };
    assert!(
        code.as_deref().unwrap_or_default().contains("/error"),
        "{code:?}"
    );
}
