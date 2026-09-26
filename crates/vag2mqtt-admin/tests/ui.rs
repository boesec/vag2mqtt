//! The management interface: every page renders, every action works as a plain form, and no
//! password or external origin ever reaches the browser.

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
use tower::ServiceExt;
use vag2mqtt_admin::{AppState, router};
use vag2mqtt_connector_api::fake::{FakeConnector, FakeHandle};
use vag2mqtt_connector_api::{ConnectorError, ConnectorRegistry, DiscoveredVehicle};
use vag2mqtt_domain::state::GeoPosition;
use vag2mqtt_domain::units::{Kilometres, Percent};
use vag2mqtt_domain::{
    AccountConnectionState, AccountId, Brand, Drivetrain, Reading, Secret, VehicleDataState,
    VehicleState, Vin,
};
use vag2mqtt_mqtt::VehicleMeta;
use vag2mqtt_persistence::{Database, MasterKeySource};
use vag2mqtt_runtime::{
    Publisher, RuntimeDeps, RuntimeError, RuntimeHandle, RuntimeSettings, StatePublishReport,
    Supervisor,
};

/// The password no page may ever render.
const MARKER: &str = "PASSWORD-MARKER-4b71";

struct NullPublisher;

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
        true
    }
    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

struct Ui {
    _dir: tempfile::TempDir,
    app: Router,
    audi: FakeHandle,
    runtime: RuntimeHandle,
    seen: Arc<Mutex<Vec<String>>>,
}

fn vin() -> Vin {
    Vin::new("WAUZZZ0000000TEST").unwrap()
}

/// A snapshot with all three reading states, which is what the diagnostics page exists for.
fn mixed_state() -> VehicleState {
    let mut state = VehicleState::unsupported(Utc::now());
    state.odometer = Reading::present(Kilometres::new(43120), Some(Utc::now()));
    state.battery.soc = Reading::present(Percent::try_new(67).unwrap(), Some(Utc::now()));
    state.battery.range = Reading::present(Kilometres::new(412), Some(Utc::now()));
    state.climatisation.state = Reading::Unavailable;
    state.position = Reading::present(
        GeoPosition::try_new(48.137154, 11.576124, None).unwrap(),
        Some(Utc::now()),
    );
    state
}

async fn start() -> Ui {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path(), MasterKeySource::Raw(Secret::new([3; 32])))
        .await
        .unwrap();
    let (audi, audi_handle) = FakeConnector::builder(Brand::Audi)
        .min_interval(Duration::from_secs(300))
        .vehicles(vec![DiscoveredVehicle {
            vin: vin(),
            model: Some("A6 e-tron".into()),
            drivetrain: Drivetrain::Electric,
        }])
        .state(vin(), mixed_state())
        .build();
    let registry = ConnectorRegistry::new().register(Arc::new(audi)).unwrap();
    let brands = registry.brands();
    let factory: vag2mqtt_runtime::PublisherFactory =
        Arc::new(|_config, _password| Ok(Arc::new(NullPublisher) as Arc<dyn Publisher>));
    let (runtime, _task) = Supervisor::start(RuntimeDeps {
        db,
        registry,
        publisher_factory: factory,
        clock: Arc::new(vag2mqtt_runtime::SystemClock),
        settings: Some(RuntimeSettings {
            default_polling_interval: Duration::from_secs(300),
            stale_check_interval: Duration::from_millis(100),
            ..RuntimeSettings::default()
        }),
    })
    .await
    .unwrap();
    Ui {
        _dir: dir,
        app: router(AppState::new(runtime.clone(), brands)),
        audi: audi_handle,
        runtime,
        seen: Arc::default(),
    }
}

impl Ui {
    async fn send(&self, method: &str, path: &str, form: Option<&str>) -> (StatusCode, String) {
        let builder = Request::builder().method(method).uri(path);
        let request = match form {
            Some(body) => builder
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(body.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = axum::body::to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .unwrap();
        let mut text = String::from_utf8_lossy(&bytes).into_owned();
        if let Some(location) = location {
            text.push_str(&format!("\n<!-- location: {location} -->"));
        }
        self.seen.lock().unwrap().push(text.clone());
        (status, text)
    }

    async fn get(&self, path: &str) -> (StatusCode, String) {
        self.send("GET", path, None).await
    }

    async fn post(&self, path: &str, form: &str) -> (StatusCode, String) {
        self.send("POST", path, Some(form)).await
    }

    /// Adds an account the way the interface does, and returns its id.
    async fn add_account(&self) -> String {
        let (status, body) = self
            .post(
                "/accounts",
                &format!(
                    "brand=audi&username=driver%40example.test&password={MARKER}&polling_interval_secs=300"
                ),
            )
            .await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
        let location = body
            .rsplit("location: ")
            .next()
            .unwrap()
            .trim_end_matches(" -->")
            .trim()
            .to_string();
        assert!(location.starts_with("/accounts/"), "{location}");
        location.trim_start_matches("/accounts/").to_string()
    }

    fn assert_no_password_rendered(&self) {
        for body in self.seen.lock().unwrap().iter() {
            assert!(
                !body.contains(MARKER),
                "the password reached a page: {body}"
            );
        }
    }

    async fn wait_for(&self, what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for: {what}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

// ----- tests ---------------------------------------------------------------------------

/// Steps 2 to 4 and step 11 of requirements section 11, through the interface alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_whole_setup_works_through_the_interface() {
    let ui = start().await;

    // Step 2: open the interface on a fresh data directory.
    let (status, body) = ui.get("/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("No account yet"), "{body}");
    assert!(body.contains("No broker configured"), "{body}");

    // Step 3: configure the broker.
    let (status, _) = ui
        .post(
            "/mqtt",
            "host=broker.local&port=1883&username=&password=&topic_prefix=cars&client_id=&keep_alive_secs=30&enabled=true",
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, body) = ui.get("/").await;
    assert!(body.contains("broker.local:1883"), "{body}");
    assert!(body.contains("cars"), "{body}");

    // Step 4: add an account. Steps 5 and 6 then happen on their own.
    let id = ui.add_account().await;
    ui.wait_for("the first fetch", || {
        ui.audi.counters().fetches_of(&vin()) >= 1
    })
    .await;
    // The connector counts a fetch before the runtime has stored it and refreshed its status.
    ui.wait_for("the stored snapshot", || {
        ui.runtime
            .current_status()
            .vehicles
            .first()
            .is_some_and(|v| v.last_state.is_some())
    })
    .await;

    let (status, body) = ui.get(&format!("/accounts/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("driver@example.test"), "{body}");

    // Steps 6 to 10: the vehicle was discovered and its values are visible.
    let (_, body) = ui.get("/").await;
    assert!(body.contains("A6 e-tron"), "{body}");
    assert!(body.contains("WAUZZZ0000000TEST"), "{body}");

    // Step 11: a change takes effect without a restart.
    let (status, _) = ui
        .post(
            &format!("/accounts/{id}"),
            "username=driver%40example.test&polling_interval_secs=600",
        )
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    ui.wait_for("the new interval", || {
        ui.runtime
            .current_status()
            .accounts
            .first()
            .is_some_and(|a| a.account.polling.interval == Duration::from_secs(600))
    })
    .await;
    let (_, body) = ui.get(&format!("/accounts/{id}")).await;
    assert!(
        body.contains("value=\"600\""),
        "the form shows the new interval"
    );

    ui.assert_no_password_rendered();
}

/// Every action is a form post answered with a redirect, so none of it needs JavaScript.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_action_works_without_javascript() {
    let ui = start().await;
    let id = ui.add_account().await;
    ui.wait_for("the first fetch", || {
        ui.audi.counters().fetches_of(&vin()) >= 1
    })
    .await;
    // The connector counts a fetch before the runtime has stored it and refreshed its status.
    ui.wait_for("the stored snapshot", || {
        ui.runtime
            .current_status()
            .vehicles
            .first()
            .is_some_and(|v| v.last_state.is_some())
    })
    .await;

    let actions: Vec<(&str, String, &str)> = vec![
        (
            "POST",
            format!("/accounts/{id}/credentials"),
            "password=another-secret",
        ),
        ("POST", format!("/accounts/{id}/enabled"), "enabled=false"),
        ("POST", format!("/accounts/{id}/enabled"), "enabled=true"),
        ("POST", format!("/accounts/{id}/reauthenticate"), ""),
        (
            "POST",
            "/vehicles/WAUZZZ0000000TEST".to_string(),
            "display_name=Family+car",
        ),
        (
            "POST",
            "/vehicles/WAUZZZ0000000TEST/enabled".to_string(),
            "enabled=false",
        ),
        (
            "POST",
            "/vehicles/WAUZZZ0000000TEST/enabled".to_string(),
            "enabled=true",
        ),
        (
            "POST",
            "/vehicles/WAUZZZ0000000TEST/refresh".to_string(),
            "",
        ),
    ];
    for (method, path, form) in actions {
        let (status, body) = ui.send(method, &path, Some(form)).await;
        assert_eq!(status, StatusCode::SEE_OTHER, "{path}: {body}");
    }

    ui.wait_for("the display name", || {
        ui.runtime
            .current_status()
            .vehicles
            .first()
            .is_some_and(|v| v.vehicle.display_name.as_deref() == Some("Family car"))
    })
    .await;
    let (_, body) = ui.get("/vehicles/WAUZZZ0000000TEST").await;
    assert!(body.contains("Family car"), "{body}");

    // Deletes are posts too, since a browser form cannot send DELETE.
    let (status, _) = ui.post("/vehicles/WAUZZZ0000000TEST/delete", "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (status, _) = ui.post(&format!("/accounts/{id}/delete"), "").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, body) = ui.get("/").await;
    assert!(body.contains("No account yet"), "{body}");

    ui.assert_no_password_rendered();
}

/// The point of the three value states, on one page.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn diagnostics_tells_the_three_states_apart() {
    let ui = start().await;
    ui.add_account().await;
    ui.wait_for("the first fetch", || {
        ui.audi.counters().fetches_of(&vin()) >= 1
    })
    .await;
    ui.wait_for("the stored snapshot", || {
        ui.runtime
            .current_status()
            .vehicles
            .first()
            .is_some_and(|v| v.last_state.is_some())
    })
    .await;

    let (status, body) = ui.get("/vehicles/WAUZZZ0000000TEST/diagnostics").await;
    assert_eq!(status, StatusCode::OK);

    assert!(body.contains("state-present"), "{body}");
    assert!(body.contains("state-unavailable"), "{body}");
    assert!(body.contains("state-unsupported"), "{body}");

    assert!(body.contains("43120"), "the odometer value is shown");
    assert!(body.contains("km"), "units are shown");
    assert!(body.contains("battery/soc"), "contract paths are shown");
    assert!(
        body.contains("fuel/range"),
        "unsupported categories are listed too"
    );

    // Coordinates are on no page.
    assert!(!body.contains("48.137"), "a coordinate reached the page");
    assert!(!body.contains("11.576"), "a coordinate reached the page");

    ui.assert_no_password_rendered();
}

/// A change made through the API shows up in the interface's poll fragment.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_overview_fragment_follows_the_api() {
    let ui = start().await;
    let (status, body) = ui.get("/fragments/overview").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("hx-get=\"/fragments/overview\""), "{body}");
    assert!(body.contains("every 5s"), "{body}");
    assert!(body.contains("No account yet"), "{body}");

    // Through the JSON API, not the interface.
    let request = Request::builder()
        .method("PUT")
        .uri("/api/mqtt")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"host":"api.example.test","topic_prefix":"viaapi"}"#,
        ))
        .unwrap();
    let response = ui.app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let (_, body) = ui.get("/fragments/overview").await;
    assert!(body.contains("api.example.test"), "{body}");
    assert!(body.contains("viaapi"), "{body}");

    ui.assert_no_password_rendered();
}

/// A failed action re-renders the page with the reason, rather than losing it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_change_explains_itself() {
    let ui = start().await;
    let id = ui.add_account().await;

    let (status, body) = ui
        .post(
            &format!("/accounts/{id}"),
            "username=driver%40example.test&polling_interval_secs=1",
        )
        .await;
    assert_eq!(status, StatusCode::OK, "no redirect when it failed");
    assert!(body.contains("minimum of 300 seconds"), "{body}");
    assert!(
        body.contains("banner bad"),
        "the reason is shown as an error"
    );

    // An unknown account and an unparseable one both answer 404 rather than a blank page.
    for path in [
        "/accounts/3f2504e0-4f89-11d3-9a0c-0305e82c3301",
        "/vehicles/nope",
    ] {
        let (status, body) = ui.get(path).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
        assert!(body.contains("Not found"), "{body}");
    }

    ui.assert_no_password_rendered();
}

/// An account in `auth_error` says so, with the category and the time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_authentication_failure_is_explained_on_the_page() {
    let ui = start().await;
    ui.audi
        .fail_login_always(ConnectorError::InvalidCredentials);
    let id = ui.add_account().await;

    ui.wait_for("the auth error", || {
        ui.runtime
            .current_status()
            .accounts
            .first()
            .is_some_and(|a| a.account.connection_state == AccountConnectionState::AuthError)
    })
    .await;

    let (status, body) = ui.get(&format!("/accounts/{id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("auth_error"), "{body}");
    assert!(body.contains("Last error"), "{body}");
    assert!(body.contains("<code>auth</code>"), "the category is shown");
    assert!(
        body.contains("will not resume on its own"),
        "the page explains why nothing retries"
    );

    ui.assert_no_password_rendered();
}

/// Nothing is loaded from the network: no CDN, no web font, no external image.
#[test]
fn no_template_references_an_external_origin() {
    let templates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("templates");
    let mut checked = 0;
    for entry in std::fs::read_dir(&templates).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        checked += 1;
        for needle in ["http://", "https://", "//cdn", "//unpkg", "//fonts"] {
            assert!(
                !text.contains(needle),
                "{} references {needle}",
                path.display()
            );
        }
    }
    assert!(checked >= 6, "only {checked} templates were checked");

    // The stylesheet must not pull anything either.
    let css = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/style.css"),
    )
    .unwrap();
    assert!(!css.contains("@import"), "the stylesheet imports something");
    assert!(!css.contains("url("), "the stylesheet loads a resource");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_assets_are_served_from_the_binary() {
    let ui = start().await;
    let (status, css) = ui.get("/assets/style.css").await;
    assert_eq!(status, StatusCode::OK);
    assert!(css.contains("--accent"), "the stylesheet is served");

    let (status, js) = ui.get("/assets/htmx.min.js").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        js.len() > 10_000,
        "htmx is served whole, got {} bytes",
        js.len()
    );
    assert!(js.contains("htmx"), "that really is htmx");
}
