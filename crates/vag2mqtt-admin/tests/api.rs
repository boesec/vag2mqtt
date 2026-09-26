//! Every endpoint against a temporary database, the fake connector and a fake publisher.
//!
//! The suite records every response body it sees and, at the end of each test, asserts that the
//! marker password appears in none of them. That is the write-only guarantee for secrets, checked
//! for real rather than per handler.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use tower::ServiceExt;
use vag2mqtt_admin::{AppState, ErrorBody, ROUTES, router};
use vag2mqtt_connector_api::fake::{FakeConnector, FakeHandle};
use vag2mqtt_connector_api::{ConnectorError, ConnectorRegistry, DiscoveredVehicle};
use vag2mqtt_domain::{
    AccountConnectionState, AccountId, Brand, Drivetrain, Secret, VehicleDataState, VehicleState,
    Vin,
};
use vag2mqtt_mqtt::VehicleMeta;
use vag2mqtt_persistence::{Database, MasterKeySource};
use vag2mqtt_runtime::{
    Publisher, RuntimeDeps, RuntimeError, RuntimeHandle, RuntimeSettings, StatePublishReport,
    Supervisor,
};

/// The password no response body may ever contain.
const MARKER: &str = "PASSWORD-MARKER-9f3c";

// ----- fakes ---------------------------------------------------------------------------

#[derive(Default)]
struct NullPublisher {
    connected: bool,
}

impl Publisher for NullPublisher {
    fn publish_state(
        &self,
        _meta: &VehicleMeta,
        _state: &VehicleState,
    ) -> Result<StatePublishReport, RuntimeError> {
        Ok(BTreeMap::new())
    }
    fn publish_vehicle_availability(
        &self,
        _vin: &Vin,
        _state: VehicleDataState,
        _now: DateTime<Utc>,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn publish_account_availability(
        &self,
        _id: &AccountId,
        _state: AccountConnectionState,
        _now: DateTime<Utc>,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn seed_last_changes(&self, _vin: &Vin, _last_changes: &BTreeMap<String, i64>) {}
    fn clear_vehicle(&self, _vin: &Vin) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn clear_account(&self, _id: &AccountId) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn set_operational(&self, _operational: bool) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn is_connected(&self) -> bool {
        self.connected
    }
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

// ----- harness -------------------------------------------------------------------------

struct Api {
    _dir: tempfile::TempDir,
    app: Router,
    audi: FakeHandle,
    runtime: RuntimeHandle,
    seen: Arc<Mutex<Vec<String>>>,
}

fn vin(suffix: &str) -> Vin {
    Vin::new(format!("WAUZZZ000000{suffix}")).unwrap()
}

fn discovered(suffix: &str) -> DiscoveredVehicle {
    DiscoveredVehicle {
        vin: vin(suffix),
        model: Some("A6 e-tron".into()),
        drivetrain: Drivetrain::Electric,
    }
}

async fn start() -> Api {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path(), MasterKeySource::Raw(Secret::new([7; 32])))
        .await
        .unwrap();
    let (audi, audi_handle) = FakeConnector::builder(Brand::Audi)
        .min_interval(Duration::from_secs(300))
        .vehicles(vec![discovered("0TEST")])
        .state(vin("0TEST"), VehicleState::unsupported(Utc::now()))
        .build();
    let registry = ConnectorRegistry::new().register(Arc::new(audi)).unwrap();
    let brands = registry.brands();
    let factory: vag2mqtt_runtime::PublisherFactory = Arc::new(|_config, _password| {
        Ok(Arc::new(NullPublisher { connected: true }) as Arc<dyn Publisher>)
    });
    let (runtime, _task) = Supervisor::start(RuntimeDeps {
        db,
        registry,
        publisher_factory: factory,
        clock: Arc::new(vag2mqtt_runtime::SystemClock),
        settings: Some(RuntimeSettings {
            // At or above the connector minimum, so an account created without an explicit
            // interval is not refused before the rest of its input is looked at.
            default_polling_interval: Duration::from_secs(300),
            stale_check_interval: Duration::from_millis(100),
            discovery_interval: Duration::from_millis(500),
            ..RuntimeSettings::default()
        }),
    })
    .await
    .unwrap();
    Api {
        _dir: dir,
        app: router(AppState::new(runtime.clone(), brands)),
        audi: audi_handle,
        runtime,
        seen: Arc::default(),
    }
}

impl Api {
    /// Sends a request and records the body, so the leak check at the end of a test sees it.
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let request = Request::builder().method(method).uri(path);
        let request = match body {
            Some(json) => request
                .header("content-type", "application/json")
                .body(Body::from(json.to_string()))
                .unwrap(),
            None => request.body(Body::empty()).unwrap(),
        };
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 4 * 1024 * 1024)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        self.seen.lock().unwrap().push(text.clone());
        let value = serde_json::from_str(&text).unwrap_or(Value::String(text));
        (status, value)
    }

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.send("GET", path, None).await
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.send("POST", path, Some(body)).await
    }

    /// Creates an account with the marker password and returns its id.
    async fn create_account(&self) -> String {
        let (status, body) = self
            .post(
                "/api/accounts",
                json!({
                    "brand": "audi",
                    "username": "driver@example.test",
                    "password": MARKER,
                    "polling_interval_secs": 300
                }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        body["id"].as_str().unwrap().to_string()
    }

    /// Fails the test if the marker password reached any response body.
    fn assert_no_password_leaked(&self) {
        for body in self.seen.lock().unwrap().iter() {
            assert!(
                !body.contains(MARKER),
                "the password reached a response body: {body}"
            );
        }
    }

    async fn wait_for(&self, what: &str, timeout: Duration, mut condition: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + timeout;
        while !condition() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for: {what}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

const T: Duration = Duration::from_secs(10);

fn as_error(body: &Value) -> ErrorBody {
    serde_json::from_value(body.clone()).expect("an error body")
}

// ----- tests ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_and_openapi_are_served() {
    let api = start().await;
    let (status, body) = api.get("/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], "ok");
    assert!(body["version"].is_string());

    let (status, body) = api.get("/api/openapi.yaml").await;
    assert_eq!(status, StatusCode::OK);
    let text = body.as_str().unwrap();
    assert!(
        text.starts_with("openapi:"),
        "{}",
        &text[..40.min(text.len())]
    );
    api.assert_no_password_leaked();
}

/// The document and the router are built from the same list, and every entry really answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_openapi_document_describes_exactly_the_routes_that_exist() {
    let document = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.yaml"),
    )
    .unwrap();
    let documented = documented_operations(&document);
    assert!(
        documented.len() > 10,
        "the parser found almost nothing, which means it is broken: {documented:?}"
    );

    let registered: Vec<String> = ROUTES
        .iter()
        .flat_map(|route| {
            route
                .methods
                .iter()
                .map(move |method| format!("{method} {}", route.path))
        })
        .collect();

    let mut documented_sorted = documented.clone();
    documented_sorted.sort();
    let mut registered_sorted = registered.clone();
    registered_sorted.sort();
    assert_eq!(
        documented_sorted, registered_sorted,
        "openapi.yaml and the router's ROUTES list disagree"
    );

    // And every registered operation is actually served: not 404, not 405.
    let api = start().await;
    for route in ROUTES {
        let path = route
            .path
            .replace("{id}", "does-not-exist")
            .replace("{vin}", "WAUZZZ0000000TEST");
        for method in route.methods {
            let body = matches!(*method, "POST" | "PUT" | "PATCH").then(|| json!({}));
            let (status, _) = api.send(method, &path, body).await;
            assert_ne!(status, StatusCode::NOT_IMPLEMENTED, "{method} {path}");
            assert_ne!(
                status,
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path} is registered but not served"
            );
        }
    }
    api.assert_no_password_leaked();
}

/// Extracts `METHOD /path` for every operation in the document's `paths:` section.
///
/// A deliberately small parser rather than a YAML dependency: the file is ours, its shape is
/// fixed, and the test above fails loudly if this returns nothing.
fn documented_operations(document: &str) -> Vec<String> {
    const METHODS: [&str; 7] = ["get", "put", "post", "delete", "patch", "head", "options"];
    let mut operations = Vec::new();
    let mut in_paths = false;
    let mut current: Option<String> = None;
    for line in document.lines() {
        if line.starts_with("paths:") {
            in_paths = true;
            continue;
        }
        if !in_paths {
            continue;
        }
        // A top level key ends the section.
        if !line.starts_with(' ') && !line.trim().is_empty() {
            break;
        }
        let indent = line.len() - line.trim_start().len();
        let trimmed = line.trim_end();
        if indent == 2 && trimmed.ends_with(':') {
            current = Some(trimmed.trim().trim_end_matches(':').to_string());
            continue;
        }
        if indent == 4
            && trimmed.ends_with(':')
            && let Some(path) = &current
        {
            let key = trimmed.trim().trim_end_matches(':');
            if METHODS.contains(&key) {
                operations.push(format!("{} {path}", key.to_uppercase()));
            }
        }
    }
    operations
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn creating_an_account_runs_the_whole_flow_and_shows_up_in_status() {
    let api = start().await;
    let id = api.create_account().await;

    // Adding an account means login, discovery and a first poll, all without a restart.
    api.wait_for("the first fetch", T, || {
        api.audi.counters().fetches_of(&vin("0TEST")) >= 1
    })
    .await;

    let (status, body) = api.get("/api/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(body["accounts"][0]["id"], id);
    assert_eq!(body["accounts"][0]["username"], "driver@example.test");
    assert_eq!(body["accounts"][0]["brand"], "audi");
    assert_eq!(body["accounts"][0]["polling_interval_secs"], 300);
    assert!(body["accounts"][0]["running"].as_bool().unwrap());
    assert_eq!(body["vehicles"].as_array().unwrap().len(), 1);
    assert_eq!(body["vehicles"][0]["vin"], "WAUZZZ0000000TEST");
    assert_eq!(body["vehicles"][0]["label"], "A6 e-tron");

    let (status, body) = api.get(&format!("/api/accounts/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], id);

    let (status, body) = api.get("/api/accounts").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);

    let (status, body) = api.get("/api/brands").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["brands"], json!(["audi"]));

    api.assert_no_password_leaked();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_writes_take_effect() {
    let api = start().await;
    let id = api.create_account().await;

    let (status, _) = api
        .send(
            "PATCH",
            &format!("/api/accounts/{id}"),
            Some(json!({"polling_interval_secs": 600})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = api.get(&format!("/api/accounts/{id}")).await;
    assert_eq!(body["polling_interval_secs"], 600);

    let (status, _) = api
        .send(
            "PUT",
            &format!("/api/accounts/{id}/credentials"),
            Some(json!({"password": MARKER})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = api
        .post(
            &format!("/api/accounts/{id}/enabled"),
            json!({"enabled": false}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    api.wait_for("the account task to stop", T, || {
        api.runtime
            .current_status()
            .accounts
            .first()
            .is_some_and(|account| !account.running)
    })
    .await;
    let (_, body) = api.get(&format!("/api/accounts/{id}")).await;
    assert_eq!(body["connection_state"], "disabled");

    let (status, _) = api
        .post(
            &format!("/api/accounts/{id}/enabled"),
            json!({"enabled": true}),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = api
        .post(&format!("/api/accounts/{id}/reauthenticate"), json!({}))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = api
        .send("DELETE", &format!("/api/accounts/{id}"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = api.get(&format!("/api/accounts/{id}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    api.assert_no_password_leaked();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn vehicle_endpoints_read_and_write() {
    let api = start().await;
    api.create_account().await;
    api.wait_for("the first fetch", T, || {
        api.audi.counters().fetches_of(&vin("0TEST")) >= 1
    })
    .await;
    let path = "/api/vehicles/WAUZZZ0000000TEST";

    let (status, body) = api.get("/api/vehicles").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_array().unwrap().len(), 1);

    let (status, body) = api.get(path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["vin"], "WAUZZZ0000000TEST");
    assert!(body["has_state"].as_bool().unwrap());

    let (status, body) = api.get(&format!("{path}/state")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["vin"], "WAUZZZ0000000TEST");
    assert!(body["state"]["fetched_at"].is_string());
    assert_eq!(body["state"]["odometer"]["state"], "unsupported");

    // The raw response endpoint exists but has nothing to show yet.
    let (status, body) = api.get(&format!("{path}/raw")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(as_error(&body).error, "not_found");

    let (status, _) = api
        .send("PATCH", path, Some(json!({"display_name": "Family car"})))
        .await;
    assert_eq!(status, StatusCode::OK);
    api.wait_for("the display name to be stored", T, || {
        api.runtime
            .current_status()
            .vehicles
            .first()
            .is_some_and(|vehicle| vehicle.vehicle.display_name.is_some())
    })
    .await;
    let (_, body) = api.get(path).await;
    assert_eq!(body["display_name"], "Family car");
    assert_eq!(body["label"], "Family car");

    let (status, _) = api
        .post(&format!("{path}/enabled"), json!({"enabled": false}))
        .await;
    assert_eq!(status, StatusCode::OK);
    api.wait_for("the vehicle to be disabled", T, || {
        api.runtime
            .current_status()
            .vehicles
            .first()
            .is_some_and(|vehicle| vehicle.vehicle.data_state == VehicleDataState::Disabled)
    })
    .await;
    let (status, _) = api
        .post(&format!("{path}/enabled"), json!({"enabled": true}))
        .await;
    assert_eq!(status, StatusCode::OK);

    let before = api.audi.counters().fetches_of(&vin("0TEST"));
    let (status, _) = api.post(&format!("{path}/refresh"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    api.wait_for("the on-demand refresh", T, || {
        api.audi.counters().fetches_of(&vin("0TEST")) > before
    })
    .await;

    let (status, _) = api.send("DELETE", path, None).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = api.get(path).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    api.assert_no_password_leaked();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mqtt_is_configured_and_switched() {
    let api = start().await;

    let (status, body) = api.get("/api/mqtt").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body["configured"].as_bool().unwrap());

    let (status, _) = api
        .send(
            "PUT",
            "/api/mqtt",
            Some(json!({
                "host": "broker.local",
                "port": 8883,
                "username": "user",
                "password": MARKER,
                "tls": true,
                "topic_prefix": "cars"
            })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = api.get("/api/mqtt").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body["configured"].as_bool().unwrap());
    assert!(body["enabled"].as_bool().unwrap());
    assert_eq!(body["config"]["host"], "broker.local");
    assert_eq!(body["config"]["port"], 8883);
    assert_eq!(body["config"]["topic_prefix"], "cars");
    assert!(body["config"]["tls"].as_bool().unwrap());
    assert!(body["config"].get("password").is_none());

    let (status, _) = api
        .post("/api/mqtt/enabled", json!({"enabled": false}))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (_, body) = api.get("/api/mqtt").await;
    assert!(!body["enabled"].as_bool().unwrap());

    api.assert_no_password_leaked();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_error_mapping_table_holds_over_http() {
    let api = start().await;
    let id = api.create_account().await;

    // 422 validation, attributed to the polling interval.
    let (status, body) = api
        .send(
            "PATCH",
            &format!("/api/accounts/{id}"),
            Some(json!({"polling_interval_secs": 1})),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let failure = as_error(&body);
    assert_eq!(failure.error, "validation");
    assert_eq!(failure.field.as_deref(), Some("polling_interval"));
    assert!(failure.message.contains("300"), "{}", failure.message);

    // 422 validation, attributed to the brand.
    let (status, body) = api
        .post(
            "/api/accounts",
            json!({"brand": "cupra", "username": "x@example.test", "password": MARKER}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(as_error(&body).field.as_deref(), Some("brand"));

    // 422 validation without a field.
    let (status, body) = api
        .post(
            "/api/accounts",
            json!({"brand": "audi", "username": "   ", "password": MARKER}),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let failure = as_error(&body);
    assert_eq!(failure.error, "validation");
    assert!(failure.field.is_none());

    // 404 not found, for a well formed id and for an unparseable one.
    for path in [
        "/api/accounts/3f2504e0-4f89-11d3-9a0c-0305e82c3301",
        "/api/accounts/not%20an%20id",
        "/api/vehicles/WAUZZZ0000000NEXT",
        "/api/vehicles/nope",
    ] {
        let (status, body) = api.get(path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert_eq!(as_error(&body).error, "not_found", "{path}");
    }

    // 400 bad request: malformed JSON.
    let request = Request::builder()
        .method("POST")
        .uri("/api/accounts")
        .header("content-type", "application/json")
        .body(Body::from("{not json"))
        .unwrap();
    let response = api.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let bytes = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(as_error(&body).error, "bad_request");

    api.assert_no_password_leaked();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_state_is_404_until_the_first_fetch_and_an_auth_error_is_visible() {
    let api = start().await;
    api.audi
        .fail_login_always(ConnectorError::InvalidCredentials);
    let id = api.create_account().await;

    let (status, body) = api.get("/api/vehicles/WAUZZZ0000000TEST/state").await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    api.wait_for("the account to reach auth_error", T, || {
        api.runtime
            .current_status()
            .accounts
            .first()
            .is_some_and(|a| a.account.connection_state == AccountConnectionState::AuthError)
    })
    .await;
    let (_, body) = api.get(&format!("/api/accounts/{id}")).await;
    assert_eq!(body["connection_state"], "auth_error");
    assert_eq!(body["last_error"]["category"], "auth");
    assert!(body["last_error"]["message"].is_string());

    api.assert_no_password_leaked();
}
