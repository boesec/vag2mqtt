//! Supervisor behaviour against the fake connector, a temporary database and a recording
//! publisher. Real time with short settings; the backoff arithmetic itself is unit tested.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use vag2mqtt_connector_api::fake::{FakeConnector, FakeHandle};
use vag2mqtt_connector_api::{ConnectorError, ConnectorRegistry, DiscoveredVehicle};
use vag2mqtt_domain::{
    AccountConnectionState, AccountId, Brand, Drivetrain, ErrorCategory, MqttConfig, PollingConfig,
    Secret, VehicleDataState, VehicleState, Vin,
};
use vag2mqtt_mqtt::VehicleMeta;
use vag2mqtt_persistence::{Database, MasterKeySource};
use vag2mqtt_runtime::{
    AccountCreate, AccountUpdate, Clock, Publisher, RuntimeDeps, RuntimeError, RuntimeHandle,
    RuntimeSettings, StatePublishReport, Supervisor,
};

// ----- recording publisher ------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Event {
    State(Vin),
    VehicleAvailability(Vin, VehicleDataState),
    AccountAvailability(AccountId, AccountConnectionState),
    ClearedVehicle(Vin),
    ClearedAccount(AccountId),
    Operational(bool),
    Shutdown,
}

#[derive(Clone, Default)]
struct Recorder {
    events: Arc<Mutex<Vec<Event>>>,
    connected: Arc<AtomicBool>,
}

impl Recorder {
    fn events(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }

    fn push(&self, event: Event) {
        self.events.lock().unwrap().push(event);
    }

    fn count(&self, pred: impl Fn(&Event) -> bool) -> usize {
        self.events().iter().filter(|e| pred(e)).count()
    }
}

impl Publisher for Recorder {
    fn publish_state(
        &self,
        meta: &VehicleMeta,
        _state: &VehicleState,
    ) -> Result<StatePublishReport, RuntimeError> {
        self.push(Event::State(meta.vin.clone()));
        Ok(BTreeMap::from([("odometer".to_string(), 1_i64)]))
    }

    fn publish_vehicle_availability(
        &self,
        vin: &Vin,
        state: VehicleDataState,
        _now: DateTime<Utc>,
    ) -> Result<(), RuntimeError> {
        self.push(Event::VehicleAvailability(vin.clone(), state));
        Ok(())
    }

    fn publish_account_availability(
        &self,
        id: &AccountId,
        state: AccountConnectionState,
        _now: DateTime<Utc>,
    ) -> Result<(), RuntimeError> {
        self.push(Event::AccountAvailability(id.clone(), state));
        Ok(())
    }

    fn seed_last_changes(&self, _vin: &Vin, _last_changes: &BTreeMap<String, i64>) {}

    fn clear_vehicle(&self, vin: &Vin) -> Result<(), RuntimeError> {
        self.push(Event::ClearedVehicle(vin.clone()));
        Ok(())
    }

    fn clear_account(&self, id: &AccountId) -> Result<(), RuntimeError> {
        self.push(Event::ClearedAccount(id.clone()));
        Ok(())
    }

    fn set_operational(&self, operational: bool) -> Result<(), RuntimeError> {
        self.push(Event::Operational(operational));
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    fn shutdown(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.push(Event::Shutdown);
        self.connected.store(false, Ordering::SeqCst);
        Box::pin(async {})
    }
}

// ----- harness --------------------------------------------------------------------------

#[derive(Clone)]
struct ManualClock(Arc<Mutex<DateTime<Utc>>>);

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    db: Database,
    handle: RuntimeHandle,
    audi: FakeHandle,
    recorders: Arc<Mutex<Vec<Recorder>>>,
    clock: ManualClock,
}

impl Harness {
    fn recorder(&self, index: usize) -> Recorder {
        self.recorders.lock().unwrap()[index].clone()
    }

    async fn account_state(&self, id: &AccountId) -> AccountConnectionState {
        self.db
            .accounts()
            .get(id)
            .await
            .unwrap()
            .unwrap()
            .connection_state
    }
}

fn test_settings() -> RuntimeSettings {
    RuntimeSettings {
        default_polling_interval: Duration::from_millis(200),
        backoff_initial: Duration::from_millis(100),
        backoff_max: Duration::from_millis(400),
        backoff_factor: 2.0,
        backoff_jitter: 0.0,
        failures_to_error: 3,
        rate_limit_pause: Duration::from_millis(500),
        stale_multiplier: 2,
        discovery_interval: Duration::from_millis(300),
        respawn_delay: Duration::from_millis(100),
        respawns_per_hour: 5,
        stale_check_interval: Duration::from_millis(100),
    }
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

fn state() -> VehicleState {
    VehicleState::unsupported(Utc::now())
}

/// Starts a supervisor with an Audi fake (vehicles `0TEST`, optionally more), an MQTT config
/// pointing at the recording factory, and the given extra connectors.
async fn start(
    configure_audi: impl FnOnce(
        vag2mqtt_connector_api::fake::FakeConnectorBuilder,
    ) -> vag2mqtt_connector_api::fake::FakeConnectorBuilder,
    extra: Vec<Arc<dyn vag2mqtt_connector_api::Connector>>,
    with_mqtt: bool,
) -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path(), MasterKeySource::Raw(Secret::new([1; 32])))
        .await
        .unwrap();
    if with_mqtt {
        db.mqtt()
            .upsert(&MqttConfig::defaults("broker.test", "vag2mqtt-test"))
            .await
            .unwrap();
    }
    let builder = FakeConnector::builder(Brand::Audi)
        .min_interval(Duration::from_millis(50))
        .vehicles(vec![discovered("0TEST")])
        .state(vin("0TEST"), state());
    let (audi, audi_handle) = configure_audi(builder).build();
    let mut registry = ConnectorRegistry::new().register(Arc::new(audi)).unwrap();
    for connector in extra {
        registry = registry.register(connector).unwrap();
    }
    let recorders: Arc<Mutex<Vec<Recorder>>> = Arc::default();
    let factory_recorders = Arc::clone(&recorders);
    let factory: vag2mqtt_runtime::PublisherFactory = Arc::new(move |_config, _password| {
        let recorder = Recorder::default();
        recorder.connected.store(true, Ordering::SeqCst);
        factory_recorders.lock().unwrap().push(recorder.clone());
        Ok(Arc::new(recorder) as Arc<dyn Publisher>)
    });
    let clock = ManualClock(Arc::new(Mutex::new(
        Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0)
            .single()
            .unwrap(),
    )));
    let (handle, _task) = Supervisor::start(RuntimeDeps {
        db: db.clone(),
        registry,
        publisher_factory: factory,
        clock: Arc::new(clock.clone()),
        settings: Some(test_settings()),
    })
    .await
    .unwrap();
    Harness {
        _dir: dir,
        db,
        handle,
        audi: audi_handle,
        recorders,
        clock,
    }
}

async fn create(handle: &RuntimeHandle, brand: Brand, interval: Duration) -> AccountId {
    handle
        .create_account(AccountCreate {
            brand,
            username: format!("{brand}@example.test"),
            password: Secret::new("pw".into()),
            polling: Some(PollingConfig { interval }),
            enabled: true,
        })
        .await
        .unwrap()
}

async fn wait_for(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + timeout;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met within {timeout:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_for_async<F, Fut>(timeout: Duration, mut condition: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    while !condition().await {
        assert!(
            tokio::time::Instant::now() < deadline,
            "condition not met within {timeout:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

const T: Duration = Duration::from_secs(10);

// ----- tests -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_account_runs_the_full_flow_and_replies_after_commit() {
    let h = start(|b| b, vec![], true).await;
    let id = create(&h.handle, Brand::Audi, Duration::from_secs(60)).await;
    // Rows exist when the reply arrives.
    assert!(h.db.accounts().get(&id).await.unwrap().is_some());

    let recorder = h.recorder(0);
    wait_for(T, || {
        recorder.count(|e| e == &Event::State(vin("0TEST"))) >= 1
    })
    .await;
    let vehicles = h.db.vehicles().list_for_account(&id).await.unwrap();
    assert_eq!(vehicles.len(), 1);
    wait_for_async(T, || async {
        h.db.vehicles()
            .get(&vin("0TEST"))
            .await
            .unwrap()
            .unwrap()
            .data_state
            == VehicleDataState::Fresh
    })
    .await;
    wait_for_async(T, || async {
        h.account_state(&id).await == AccountConnectionState::Ok
    })
    .await;
    let counters = h.audi.counters();
    assert_eq!(counters.logins, 1);
    assert_eq!(counters.lists, 1);
    assert!(counters.fetches_of(&vin("0TEST")) >= 1);
    assert!(recorder.events().contains(&Event::VehicleAvailability(
        vin("0TEST"),
        VehicleDataState::Fresh
    )));
    assert!(h.db.secrets().account_session(&id).await.unwrap().is_some());
    let status = h.handle.current_status();
    assert_eq!(status.accounts.len(), 1);
    assert!(status.accounts[0].running);
    assert!(status.vehicles[0].last_state.is_some());
    h.handle.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_account_does_not_block_another() {
    let (skoda, skoda_handle) = FakeConnector::builder(Brand::Skoda)
        .min_interval(Duration::from_millis(50))
        .fail_login_always(ConnectorError::InvalidCredentials)
        .build();
    let h = start(|b| b, vec![Arc::new(skoda)], true).await;
    let bad = create(&h.handle, Brand::Skoda, Duration::from_millis(200)).await;
    let good = create(&h.handle, Brand::Audi, Duration::from_millis(200)).await;

    wait_for_async(T, || async {
        h.account_state(&bad).await == AccountConnectionState::AuthError
    })
    .await;
    let recorder = h.recorder(0);
    wait_for(T, || {
        recorder.count(|e| e == &Event::State(vin("0TEST"))) >= 3
    })
    .await;
    assert_eq!(h.account_state(&good).await, AccountConnectionState::Ok);
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        skoda_handle.counters().logins,
        1,
        "no automatic retry after an auth error"
    );
    let error =
        h.db.accounts()
            .get(&bad)
            .await
            .unwrap()
            .unwrap()
            .last_error
            .unwrap();
    assert_eq!(error.category, ErrorCategory::Auth);

    // The manual retry (ER-005) logs in again.
    skoda_handle.clear_login_failure();
    h.handle.reauthenticate(&bad).await.unwrap();
    wait_for(T, || skoda_handle.counters().logins >= 2).await;
    h.handle.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interval_change_takes_effect_without_restart() {
    let h = start(|b| b, vec![], true).await;
    let id = create(&h.handle, Brand::Audi, Duration::from_secs(5)).await;
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TEST")) == 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.audi.counters().fetches_of(&vin("0TEST")), 1);

    h.handle
        .update_account(
            &id,
            AccountUpdate {
                username: None,
                polling: Some(PollingConfig {
                    interval: Duration::from_millis(200),
                }),
            },
        )
        .await
        .unwrap();
    let before = tokio::time::Instant::now();
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TEST")) >= 2).await;
    assert!(
        before.elapsed() < Duration::from_secs(2),
        "the new interval applied"
    );
    let times = h.audi.fetch_times();
    let gap = times[times.len() - 1].1 - times[times.len() - 2].1;
    assert!(gap < Duration::from_secs(2), "{gap:?}");

    // Below the connector minimum is refused.
    let err = h
        .handle
        .update_account(
            &id,
            AccountUpdate {
                username: None,
                polling: Some(PollingConfig {
                    interval: Duration::from_millis(10),
                }),
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, RuntimeError::IntervalBelowMinimum { .. }));
    h.handle.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_vehicle_is_marked_not_deleted() {
    let h = start(
        |b| {
            b.vehicles(vec![discovered("0TEST"), discovered("0TW00")])
                .state(vin("0TW00"), state())
        },
        vec![],
        true,
    )
    .await;
    let id = create(&h.handle, Brand::Audi, Duration::from_millis(100)).await;
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TW00")) >= 1).await;

    h.audi.set_vehicles(vec![discovered("0TEST")]);
    wait_for_async(T, || async {
        h.db.vehicles()
            .get(&vin("0TW00"))
            .await
            .unwrap()
            .unwrap()
            .missing_since
            .is_some()
    })
    .await;
    let fetches_at_mark = h.audi.counters().fetches_of(&vin("0TW00"));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        h.audi.counters().fetches_of(&vin("0TW00")),
        fetches_at_mark,
        "missing vehicles are not polled"
    );
    assert!(h.audi.counters().fetches_of(&vin("0TEST")) > 2);
    let vehicles = h.db.vehicles().list_for_account(&id).await.unwrap();
    assert_eq!(vehicles.len(), 2, "not deleted");
    let missing = vehicles.iter().find(|v| v.vin == vin("0TW00")).unwrap();
    assert_eq!(missing.data_state, VehicleDataState::Unavailable);
    assert!(h.recorder(0).events().contains(&Event::VehicleAvailability(
        vin("0TW00"),
        VehicleDataState::Unavailable
    )));
    h.handle.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn transient_failures_back_off_and_flip_to_error_while_polling_continues() {
    let h = start(|b| b, vec![], true).await;
    let id = create(&h.handle, Brand::Audi, Duration::from_millis(100)).await;
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TEST")) >= 1).await;
    h.audi
        .fail_fetch_always(vin("0TEST"), ConnectorError::network("connection reset"));

    wait_for_async(T, || async {
        h.account_state(&id).await == AccountConnectionState::Error
    })
    .await;
    let account = h.db.accounts().get(&id).await.unwrap().unwrap();
    assert_eq!(account.last_error.unwrap().category, ErrorCategory::Network);
    wait_for(T, || {
        h.handle.current_status().accounts[0].consecutive_failures >= 3
    })
    .await;

    let fetches = h.audi.counters().fetches_of(&vin("0TEST"));
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TEST")) > fetches).await;

    // Backoff grows: the gaps between the failing fetches increase (100 ms, 200 ms, 400 ms cap).
    let times = h.audi.fetch_times();
    let gaps: Vec<Duration> = times.windows(2).map(|w| w[1].1 - w[0].1).collect();
    assert!(gaps.len() >= 3, "{gaps:?}");
    assert!(gaps[1] >= Duration::from_millis(90), "{gaps:?}");
    assert!(gaps[2] >= Duration::from_millis(180), "{gaps:?}");

    // Recovery clears the error.
    h.audi.clear_fetch_failure(&vin("0TEST"));
    wait_for_async(T, || async {
        h.account_state(&id).await == AccountConnectionState::Ok
    })
    .await;
    assert_eq!(
        h.handle.current_status().accounts[0].consecutive_failures,
        0
    );
    h.handle.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limit_pauses_the_whole_account() {
    let h = start(|b| b, vec![], true).await;
    let id = create(&h.handle, Brand::Audi, Duration::from_millis(100)).await;
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TEST")) >= 1).await;
    h.audi.fail_fetch_once(
        vin("0TEST"),
        ConnectorError::RateLimited {
            retry_after: Some(Duration::from_millis(700)),
        },
    );
    wait_for_async(T, || async {
        h.account_state(&id).await == AccountConnectionState::RateLimited
    })
    .await;
    assert!(h.recorder(0).events().contains(&Event::AccountAvailability(
        id.clone(),
        AccountConnectionState::RateLimited
    )));
    let paused_at = h.audi.fetch_times().last().unwrap().1;
    wait_for_async(T, || async {
        h.account_state(&id).await == AccountConnectionState::Ok
    })
    .await;
    let resumed_at = h.audi.fetch_times().last().unwrap().1;
    assert!(
        resumed_at - paused_at >= Duration::from_millis(650),
        "{:?}",
        resumed_at - paused_at
    );
    h.handle.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_expiry_triggers_one_relogin() {
    let h = start(|b| b, vec![], true).await;
    let _id = create(&h.handle, Brand::Audi, Duration::from_millis(100)).await;
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TEST")) >= 1).await;
    assert_eq!(h.audi.counters().logins, 1);
    h.audi
        .fail_fetch_once(vin("0TEST"), ConnectorError::SessionExpired);
    let fetches = h.audi.counters().fetches_of(&vin("0TEST"));
    wait_for(T, || {
        h.audi.counters().fetches_of(&vin("0TEST")) >= fetches + 2
    })
    .await;
    assert_eq!(h.audi.counters().logins, 2);
    h.handle.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_panicking_task_is_respawned_and_the_supervisor_survives() {
    let h = start(|b| b, vec![], true).await;
    let id = create(&h.handle, Brand::Audi, Duration::from_millis(100)).await;
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TEST")) >= 1).await;
    h.audi.panic_on_next_fetch(vin("0TEST"));
    wait_for(T, || h.handle.current_status().accounts[0].respawns >= 1).await;
    let fetches = h.audi.counters().fetches_of(&vin("0TEST"));
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TEST")) > fetches).await;
    wait_for_async(T, || async {
        h.account_state(&id).await == AccountConnectionState::Ok
    })
    .await;
    assert!(h.handle.current_status().accounts[0].running);
    h.handle.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deletes_clear_retained_topics_before_rows_vanish() {
    let h = start(
        |b| {
            b.vehicles(vec![discovered("0TEST"), discovered("0TW00")])
                .state(vin("0TW00"), state())
        },
        vec![],
        true,
    )
    .await;
    let id = create(&h.handle, Brand::Audi, Duration::from_millis(100)).await;
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TW00")) >= 1).await;

    // The manufacturer stops listing the second vehicle; the user then removes it explicitly.
    h.audi.set_vehicles(vec![discovered("0TEST")]);
    wait_for_async(T, || async {
        h.db.vehicles()
            .get(&vin("0TW00"))
            .await
            .unwrap()
            .unwrap()
            .missing_since
            .is_some()
    })
    .await;
    h.handle.delete_vehicle(&vin("0TW00")).await.unwrap();
    let recorder = h.recorder(0);
    assert!(
        recorder
            .events()
            .contains(&Event::ClearedVehicle(vin("0TW00")))
    );
    assert!(h.db.vehicles().get(&vin("0TW00")).await.unwrap().is_none());
    let fetches = h.audi.counters().fetches_of(&vin("0TW00"));
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(h.audi.counters().fetches_of(&vin("0TW00")), fetches);
    assert!(
        h.db.vehicles().get(&vin("0TW00")).await.unwrap().is_none(),
        "not rediscovered"
    );

    h.handle.delete_account(&id).await.unwrap();
    let events = recorder.events();
    assert!(events.contains(&Event::ClearedVehicle(vin("0TEST"))));
    assert!(events.contains(&Event::ClearedAccount(id.clone())));
    assert!(h.db.accounts().get(&id).await.unwrap().is_none());
    assert!(h.db.vehicles().get(&vin("0TEST")).await.unwrap().is_none());
    assert!(h.handle.current_status().accounts.is_empty());
    h.handle.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mqtt_reconfiguration_swaps_the_publisher_and_republishes() {
    let h = start(|b| b, vec![], true).await;
    let _id = create(&h.handle, Brand::Audi, Duration::from_secs(60)).await;
    let first = h.recorder(0);
    wait_for(T, || first.count(|e| matches!(e, Event::State(_))) >= 1).await;

    let mut config = MqttConfig::defaults("other.test", "vag2mqtt-2");
    config.topic_prefix = "cars".into();
    h.handle.configure_mqtt(config, None).await.unwrap();
    assert!(first.events().contains(&Event::Shutdown));
    assert_eq!(h.recorders.lock().unwrap().len(), 2);
    let second = h.recorder(1);
    wait_for(T, || {
        second.count(|e| e == &Event::State(vin("0TEST"))) >= 1
    })
    .await;
    assert!(second.events().contains(&Event::VehicleAvailability(
        vin("0TEST"),
        VehicleDataState::Fresh
    )));
    assert_eq!(
        h.db.mqtt().get().await.unwrap().unwrap().topic_prefix,
        "cars"
    );

    h.handle.set_mqtt_enabled(false).await.unwrap();
    assert!(second.events().contains(&Event::Shutdown));
    assert!(!h.handle.current_status().mqtt.enabled);
    h.handle.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn polling_continues_without_mqtt_and_stale_is_detected() {
    let h = start(|b| b, vec![], false).await;
    let _id = create(&h.handle, Brand::Audi, Duration::from_secs(60)).await;
    wait_for_async(T, || async {
        h.db.vehicles()
            .get(&vin("0TEST"))
            .await
            .unwrap()
            .is_some_and(|v| v.data_state == VehicleDataState::Fresh)
    })
    .await;
    assert!(h.recorders.lock().unwrap().is_empty());
    assert!(
        h.db.vehicle_states()
            .get(&vin("0TEST"))
            .await
            .unwrap()
            .is_some()
    );

    // 60 s interval × stale multiplier 2 = 2 min; move the wall clock by 5 min.
    *h.clock.0.lock().unwrap() += chrono::Duration::minutes(5);
    wait_for_async(T, || async {
        h.db.vehicles()
            .get(&vin("0TEST"))
            .await
            .unwrap()
            .is_some_and(|v| v.data_state == VehicleDataState::Stale)
    })
    .await;
    h.handle.shutdown().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabling_and_enabling_account_and_vehicle() {
    let h = start(|b| b, vec![], true).await;
    let id = create(&h.handle, Brand::Audi, Duration::from_millis(100)).await;
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TEST")) >= 1).await;

    h.handle
        .set_vehicle_enabled(&vin("0TEST"), false)
        .await
        .unwrap();
    wait_for_async(T, || async {
        h.db.vehicles()
            .get(&vin("0TEST"))
            .await
            .unwrap()
            .unwrap()
            .data_state
            == VehicleDataState::Disabled
    })
    .await;
    let fetches = h.audi.counters().fetches_of(&vin("0TEST"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.audi.counters().fetches_of(&vin("0TEST")), fetches);
    h.handle
        .set_vehicle_enabled(&vin("0TEST"), true)
        .await
        .unwrap();
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TEST")) > fetches).await;

    h.handle.set_account_enabled(&id, false).await.unwrap();
    assert_eq!(h.account_state(&id).await, AccountConnectionState::Disabled);
    assert!(!h.handle.current_status().accounts[0].running);
    assert!(h.recorder(0).events().contains(&Event::AccountAvailability(
        id.clone(),
        AccountConnectionState::Disabled
    )));
    let fetches = h.audi.counters().fetches_of(&vin("0TEST"));
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(h.audi.counters().fetches_of(&vin("0TEST")), fetches);

    h.handle.set_account_enabled(&id, true).await.unwrap();
    wait_for(T, || h.audi.counters().fetches_of(&vin("0TEST")) > fetches).await;
    assert!(h.handle.current_status().accounts[0].running);

    // Refresh now polls once immediately.
    let fetches = h.audi.counters().fetches_of(&vin("0TEST"));
    h.handle.refresh_vehicle_now(&vin("0TEST")).await.unwrap();
    wait_for(Duration::from_secs(2), || {
        h.audi.counters().fetches_of(&vin("0TEST")) > fetches
    })
    .await;
    h.handle.shutdown().await.unwrap();
}
