//! The whole form login against a mock identity service and backend.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use url::Url;
use vag2mqtt_connector_api::{Connector, ConnectorError, Credentials, ErrorClass};
use vag2mqtt_connector_audi::auth::form::{CODE_RESPONSE_TYPE, Endpoints, FormLoginStrategy};
use vag2mqtt_connector_audi::{AudiConnector, AudiTokens, TraceConfig};
use vag2mqtt_domain::Secret;
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const CLIENT: &str = "09b6cbec-cd19-4589-82fd-363dfa8c24da@apps_vw-dilab_com";

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

fn build_connector(server: &MockServer, trace: TraceConfig) -> AudiConnector {
    // The mounted mocks answer the classic flow; the hybrid flow has its own test below.
    build_connector_with(server, trace, CODE_RESPONSE_TYPE)
}

fn build_connector_with(
    server: &MockServer,
    trace: TraceConfig,
    response_type: &str,
) -> AudiConnector {
    let base = Url::parse(&server.uri()).unwrap();
    let endpoints = Endpoints {
        identity_base: base.clone(),
        bff_base: base,
        response_type: response_type.to_string(),
        ..Endpoints::default()
    };
    AudiConnector::with_strategy(
        Arc::new(FormLoginStrategy::with_endpoints(endpoints)),
        trace,
    )
    .unwrap()
}

/// Mounts the happy path up to (and including) the password post. `password_outcome` is the
/// response of the password post.
async fn mount_flow(server: &MockServer, password_outcome: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path("/oidc/v1/authorize"))
        .and(query_param("code_challenge_method", "S256"))
        .and(query_param("client_id", CLIENT))
        .and(query_param("response_type", CODE_RESPONSE_TYPE))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            format!("/signin-service/v1/signin/{CLIENT}@relayState=relay-fixture"),
        ))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/signin-service/v1/signin/{CLIENT}@relayState=relay-fixture"
        )))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("signin_email_form.html")))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/signin-service/v1/{CLIENT}/login/identifier"
        )))
        .and(body_string_contains("email=driver%40example.test"))
        .and(body_string_contains("hmac=hmac-fixture"))
        .and(body_string_contains("registerFlow=false"))
        .respond_with(ResponseTemplate::new(303).insert_header(
            "Location",
            format!("/signin-service/v1/{CLIENT}/login/authenticate?relayState=relay-fixture"),
        ))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!(
            "/signin-service/v1/{CLIENT}/login/authenticate"
        )))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(fixture("signin_password_form.html")),
        )
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/signin-service/v1/{CLIENT}/login/authenticate"
        )))
        .and(body_string_contains("password=hunter2-MARKER"))
        .and(body_string_contains("hmac=hmac-fixture-2"))
        .respond_with(password_outcome)
        .mount(server)
        .await;
}

async fn mount_callback_chain(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/oidc/v1/oauth/sso"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", "/oidc/v1/oauth/client/callback?x=1"),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/oidc/v1/oauth/client/callback"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            "myaudi:///#state=abc&code=CODE-MARKER&id_token=ID-MARKER",
        ))
        .mount(server)
        .await;
}

async fn mount_token(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/auth/v1/idk/oidc/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=CODE-MARKER"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("token_response.json")))
        .mount(server)
        .await;
}

#[tokio::test]
async fn full_form_login_yields_a_session() {
    let server = MockServer::start().await;
    mount_flow(
        &server,
        ResponseTemplate::new(302).insert_header("Location", "/oidc/v1/oauth/sso?a=1"),
    )
    .await;
    mount_callback_chain(&server).await;
    mount_token(&server).await;

    let connector = build_connector(&server, TraceConfig::disabled());
    let session = connector.login(&credentials()).await.unwrap();
    let tokens = AudiTokens::from_session(&session).unwrap();
    assert_eq!(tokens.access.expose_secret(), "access-fixture");
    assert_eq!(tokens.strategy, "form");
    assert!(session.expires_at.is_some());
    assert!(!format!("{session:?}").contains("access-fixture"));
}

#[tokio::test]
async fn consent_page_is_posted_and_the_flow_continues() {
    let server = MockServer::start().await;
    mount_flow(
        &server,
        ResponseTemplate::new(302).insert_header("Location", "/oidc/v1/oauth/sso?a=1"),
    )
    .await;
    Mock::given(method("GET"))
        .and(path("/oidc/v1/oauth/sso"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/consent/v1/terms"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/consent/v1/terms"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<html><body><form action=\"/consent/v1/terms/accept\" method=\"POST\">\
             <input type=\"hidden\" name=\"_csrf\" value=\"c\"><input type=\"checkbox\" name=\"accepted\" value=\"true\">\
             <button type=\"submit\">Ok</button></form></body></html>",
        ))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/consent/v1/terms/accept"))
        .and(body_string_contains("_csrf=c"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", "myaudi:///?code=CODE-MARKER&state=abc"),
        )
        .mount(&server)
        .await;
    mount_token(&server).await;

    let connector = build_connector(&server, TraceConfig::disabled());
    assert!(connector.login(&credentials()).await.is_ok());
}

#[tokio::test]
async fn invalid_password_maps_to_invalid_credentials() {
    let server = MockServer::start().await;
    mount_flow(
        &server,
        ResponseTemplate::new(302).insert_header(
            "Location",
            format!(
                "/signin-service/v1/{CLIENT}/login/authenticate?error=login.errors.password_invalid"
            ),
        ),
    )
    .await;
    let connector = build_connector(&server, TraceConfig::disabled());
    let error = connector.login(&credentials()).await.unwrap_err();
    assert_eq!(error, ConnectorError::InvalidCredentials);
    assert_eq!(error.class(), ErrorClass::AuthInvalid);
}

#[tokio::test]
async fn throttled_maps_to_rate_limit_not_credentials() {
    let server = MockServer::start().await;
    mount_flow(
        &server,
        ResponseTemplate::new(302).insert_header(
            "Location",
            "/signin-service/v1/x/login/authenticate?error=login.error.throttled",
        ),
    )
    .await;
    let connector = build_connector(&server, TraceConfig::disabled());
    let error = connector.login(&credentials()).await.unwrap_err();
    assert_eq!(error, ConnectorError::RateLimited { retry_after: None });
}

#[tokio::test]
async fn a_200_page_where_a_redirect_was_expected_is_a_parsing_error_with_the_step() {
    let server = MockServer::start().await;
    mount_flow(
        &server,
        ResponseTemplate::new(200).set_body_string("<html><body>Something new</body></html>"),
    )
    .await;
    let connector = build_connector(&server, TraceConfig::disabled());
    let error = connector.login(&credentials()).await.unwrap_err();
    assert_eq!(
        error,
        ConnectorError::Parsing {
            context: "redirect_chase"
        }
    );
    assert_eq!(error.class(), ErrorClass::Permanent);
}

#[tokio::test]
async fn captcha_and_two_factor_pages_are_reported_as_such() {
    let server = MockServer::start().await;
    mount_flow(
        &server,
        ResponseTemplate::new(200)
            .set_body_string("<html><body><div class=\"g-recaptcha\"></div></body></html>"),
    )
    .await;
    let connector = build_connector(&server, TraceConfig::disabled());
    assert_eq!(
        connector.login(&credentials()).await.unwrap_err(),
        ConnectorError::CaptchaRequired
    );

    let server = MockServer::start().await;
    mount_flow(
        &server,
        ResponseTemplate::new(200)
            .set_body_string("<html><body>Enter the verification code we sent you</body></html>"),
    )
    .await;
    let connector = build_connector(&server, TraceConfig::disabled());
    assert_eq!(
        connector.login(&credentials()).await.unwrap_err(),
        ConnectorError::TwoFactorRequired
    );
}

#[tokio::test]
async fn authorize_answering_200_html_is_reported_in_the_authorize_step() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/oidc/v1/authorize"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><body>new login ui</body></html>"),
        )
        .mount(&server)
        .await;
    let connector = build_connector(&server, TraceConfig::disabled());
    let error = connector.login(&credentials()).await.unwrap_err();
    assert_eq!(
        error,
        ConnectorError::Parsing {
            context: "signin_form"
        }
    );
}

#[tokio::test]
async fn backend_5xx_at_the_token_exchange_is_transient() {
    let server = MockServer::start().await;
    mount_flow(
        &server,
        ResponseTemplate::new(302).insert_header("Location", "/oidc/v1/oauth/sso?a=1"),
    )
    .await;
    mount_callback_chain(&server).await;
    Mock::given(method("POST"))
        .and(path("/auth/v1/idk/oidc/token"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let connector = build_connector(&server, TraceConfig::disabled());
    let error = connector.login(&credentials()).await.unwrap_err();
    assert_eq!(error.class(), ErrorClass::Transient);
    assert_eq!(
        error,
        ConnectorError::Manufacturer {
            status: Some(503),
            code: None
        }
    );
}

#[tokio::test]
async fn token_exchange_400_carries_the_error_code() {
    let server = MockServer::start().await;
    mount_flow(
        &server,
        ResponseTemplate::new(302).insert_header("Location", "/oidc/v1/oauth/sso?a=1"),
    )
    .await;
    mount_callback_chain(&server).await;
    Mock::given(method("POST"))
        .and(path("/auth/v1/idk/oidc/token"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":"invalid_request","error_description":"invalid assertion headers"}"#,
        ))
        .mount(&server)
        .await;
    let connector = build_connector(&server, TraceConfig::disabled());
    let error = connector.login(&credentials()).await.unwrap_err();
    assert_eq!(
        error,
        ConnectorError::Manufacturer {
            status: Some(400),
            code: Some("invalid_request".into())
        }
    );
}

#[tokio::test]
async fn expired_session_is_renewed_through_refresh_without_a_password_login() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth/v1/idk/oidc/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=REFRESH-MARKER"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"access_token":"new-access","expires_in":1800}"#),
        )
        .expect(1)
        .mount(&server)
        .await;
    // Any identity request would mean a password login happened: forbid it.
    Mock::given(method("GET"))
        .and(path("/oidc/v1/authorize"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let connector = build_connector(&server, TraceConfig::disabled());
    let expired = AudiTokens {
        access: Secret::new("old-access".into()),
        refresh: Some(Secret::new("REFRESH-MARKER".into())),
        id_token: None,
        expires_at: chrono::Utc::now() - chrono::Duration::minutes(5),
        strategy: "form".into(),
    };
    let mut session = expired.into_session();
    connector.refresh(&mut session).await.unwrap();
    let renewed = AudiTokens::from_session(&session).unwrap();
    assert_eq!(renewed.access.expose_secret(), "new-access");
    assert_eq!(
        renewed.refresh.as_ref().map(|t| t.expose_secret().as_str()),
        Some("REFRESH-MARKER"),
        "kept when not repeated"
    );
    assert!(session.expires_at.unwrap() > chrono::Utc::now());
}

#[tokio::test]
async fn rejected_refresh_is_session_expired() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth/v1/idk/oidc/token"))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"error":"invalid_grant"}"#))
        .mount(&server)
        .await;
    let connector = build_connector(&server, TraceConfig::disabled());
    let mut session = AudiTokens {
        access: Secret::new("a".into()),
        refresh: Some(Secret::new("r".into())),
        id_token: None,
        expires_at: chrono::Utc::now(),
        strategy: "form".into(),
    }
    .into_session();
    assert_eq!(
        connector.refresh(&mut session).await.unwrap_err(),
        ConnectorError::SessionExpired
    );
}

#[tokio::test]
async fn trace_files_contain_no_secret_and_name_every_step() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mount_flow(
        &server,
        ResponseTemplate::new(302).insert_header("Location", "/oidc/v1/oauth/sso?a=1"),
    )
    .await;
    mount_callback_chain(&server).await;
    mount_token(&server).await;

    let connector = build_connector(&server, TraceConfig::into_directory(dir.path()));
    connector.login(&credentials()).await.unwrap();

    let attempt = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap();
    let mut names = Vec::new();
    let mut all = String::new();
    for entry in std::fs::read_dir(&attempt).unwrap() {
        let path = entry.unwrap().path();
        names.push(path.file_name().unwrap().to_string_lossy().into_owned());
        all.push_str(&std::fs::read_to_string(&path).unwrap());
    }
    names.sort();
    assert!(
        names.iter().any(|n| n.starts_with("01-authorize")),
        "{names:?}"
    );
    assert!(
        names.iter().any(|n| n.starts_with("03-identifier_post")),
        "{names:?}"
    );
    assert!(
        names.iter().any(|n| n.starts_with("04-password_post")),
        "{names:?}"
    );
    assert!(
        names.iter().any(|n| n.starts_with("07-token_exchange")),
        "{names:?}"
    );
    assert!(names.contains(&"summary.txt".to_string()));
    assert!(names.contains(&"README.txt".to_string()));
    for marker in [
        "hunter2-MARKER",
        "CODE-MARKER",
        "ID-MARKER",
        "access-fixture",
        "refresh-fixture",
        "driver@example.test",
        "driver%40example.test",
    ] {
        assert!(!all.contains(marker), "{marker} leaked into the trace");
    }
    assert!(all.contains("07 token_exchange   ok"));
}

#[tokio::test]
async fn without_the_trace_variable_nothing_is_written() {
    let dir = tempfile::tempdir().unwrap();
    let server = MockServer::start().await;
    mount_flow(
        &server,
        ResponseTemplate::new(302).insert_header("Location", "/oidc/v1/oauth/sso?a=1"),
    )
    .await;
    mount_callback_chain(&server).await;
    mount_token(&server).await;
    let connector = build_connector(&server, TraceConfig::disabled());
    connector.login(&credentials()).await.unwrap();
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

/// The hybrid flow: the callback fragment carries the tokens and the token endpoint is never
/// touched. This is the route the 2026-09-23 spike showed to be necessary, because the Cariad
/// backend refuses a third party's code exchange with `invalid assertion headers`.
#[tokio::test]
async fn hybrid_flow_takes_its_tokens_from_the_callback() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/oidc/v1/authorize"))
        .and(query_param("response_type", "code token id_token"))
        .and(query_param("code_challenge_method", "S256"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "/signin-service/v1/signin/x"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/signin-service/v1/signin/x"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fixture("signin_email_form.html")))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/signin-service/v1/{CLIENT}/login/identifier"
        )))
        .respond_with(ResponseTemplate::new(303).insert_header("Location", "/password"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/password"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(fixture("live_signin_password_2026-09-23.html")),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(format!(
            "/signin-service/v1/{CLIENT}/login/authenticate"
        )))
        .and(body_string_contains("password=hunter2-MARKER"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            "myaudi:///#state=abc&code=CODE-MARKER&access_token=ACCESS-MARKER\
             &id_token=ID-MARKER&expires_in=3600&token_type=bearer",
        ))
        .mount(&server)
        .await;
    // The token endpoint must not be touched at all.
    Mock::given(method("POST"))
        .and(path("/auth/v1/idk/oidc/token"))
        .respond_with(
            ResponseTemplate::new(400).set_body_string(r#"{"error":"invalid assertion headers"}"#),
        )
        .expect(0)
        .mount(&server)
        .await;

    let connector = build_connector_with(&server, TraceConfig::disabled(), "code token id_token");
    let session = connector.login(&credentials()).await.unwrap();
    let tokens = AudiTokens::from_session(&session).unwrap();
    assert_eq!(tokens.access.expose_secret(), "ACCESS-MARKER");
    assert!(
        tokens.refresh.is_none(),
        "the hybrid flow brings no refresh token"
    );
    assert!(session.expires_at.is_some());
}

/// Without a refresh token there is nothing to renew, so the connector says the session is gone
/// and the runtime logs in again.
#[tokio::test]
async fn a_session_without_a_refresh_token_reports_session_expired() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/auth/v1/idk/oidc/token"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let connector = build_connector(&server, TraceConfig::disabled());
    let mut session = AudiTokens {
        access: Secret::new("a".into()),
        refresh: None,
        id_token: None,
        expires_at: chrono::Utc::now(),
        strategy: "form".into(),
    }
    .into_session();
    assert_eq!(
        connector.refresh(&mut session).await.unwrap_err(),
        ConnectorError::SessionExpired
    );
}
