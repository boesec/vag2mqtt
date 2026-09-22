//! A scripted connector for tests (feature `fake`).
//!
//! The runtime (WP-05) and the admin layer are tested against this instead of a manufacturer.
//! A [`FakeHandle`] changes the script while the connector is in use and exposes counters.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use tokio::time::Instant;
use vag2mqtt_domain::{Brand, DataSourceKind, VehicleState, Vin};

use crate::connector::{Connector, ConnectorInfo, Credentials, DiscoveredVehicle};
use crate::error::ConnectorError;
use crate::session::SessionState;

/// Call counters, readable through [`FakeHandle::counters`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    /// Successful and failed `login` calls.
    pub logins: u32,
    /// `refresh` calls.
    pub refreshes: u32,
    /// `list_vehicles` calls.
    pub lists: u32,
    /// `fetch_state` calls per VIN.
    pub fetches: HashMap<Vin, u32>,
}

impl Counters {
    /// Fetch count for one VIN, zero if never fetched.
    pub fn fetches_of(&self, vin: &Vin) -> u32 {
        self.fetches.get(vin).copied().unwrap_or(0)
    }
}

/// How a scripted failure applies.
#[derive(Clone, Debug)]
enum Failure {
    Once(VecDeque<ConnectorError>),
    Always(ConnectorError),
}

impl Failure {
    fn take(slot: &mut Option<Failure>) -> Option<ConnectorError> {
        match slot {
            None => None,
            Some(Failure::Always(error)) => Some(error.clone()),
            Some(Failure::Once(queue)) => {
                let next = queue.pop_front();
                if queue.is_empty() {
                    *slot = None;
                }
                next
            }
        }
    }

    fn push_once(slot: &mut Option<Failure>, error: ConnectorError) {
        match slot {
            Some(Failure::Once(queue)) => queue.push_back(error),
            _ => *slot = Some(Failure::Once(VecDeque::from([error]))),
        }
    }
}

#[derive(Debug, Default)]
struct Script {
    vehicles: Vec<DiscoveredVehicle>,
    states: HashMap<Vin, VehicleState>,
    session_ttl: Option<Duration>,
    login_failure: Option<Failure>,
    refresh_failure: Option<Failure>,
    list_failure: Option<Failure>,
    fetch_failures: HashMap<Vin, Option<Failure>>,
    panic_on_fetch: Option<Vin>,
    counters: Counters,
    fetch_times: Vec<(Vin, Instant)>,
    login_sequence: u32,
}

/// A connector whose behaviour is scripted by a [`FakeHandle`].
#[derive(Clone)]
pub struct FakeConnector {
    info: ConnectorInfo,
    script: Arc<Mutex<Script>>,
}

/// Changes the script and reads the counters of a [`FakeConnector`].
#[derive(Clone)]
pub struct FakeHandle {
    script: Arc<Mutex<Script>>,
}

/// Builds a [`FakeConnector`].
pub struct FakeConnectorBuilder {
    info: ConnectorInfo,
    script: Script,
}

impl FakeConnector {
    /// Starts a builder for the given brand with a 60 second minimum interval.
    pub fn builder(brand: Brand) -> FakeConnectorBuilder {
        FakeConnectorBuilder {
            info: ConnectorInfo {
                brand,
                kind: DataSourceKind::LiveApi,
                min_polling_interval: Duration::from_secs(60),
                supports_commands: false,
            },
            script: Script::default(),
        }
    }

    fn script(&self) -> MutexGuard<'_, Script> {
        // The script mutex is never held across an await; a poisoned mutex only means a test
        // panicked while holding it, and continuing with the inner state is what a test wants.
        self.script
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl FakeConnectorBuilder {
    /// Overrides the minimum polling interval.
    pub fn min_interval(mut self, interval: Duration) -> Self {
        self.info.min_polling_interval = interval;
        self
    }

    /// Declares the connector as an export source.
    pub fn kind(mut self, kind: DataSourceKind) -> Self {
        self.info.kind = kind;
        self
    }

    /// The vehicles discovery returns.
    pub fn vehicles(mut self, vehicles: Vec<DiscoveredVehicle>) -> Self {
        self.script.vehicles = vehicles;
        self
    }

    /// The state a fetch returns for the VIN.
    pub fn state(mut self, vin: Vin, state: VehicleState) -> Self {
        self.script.states.insert(vin, state);
        self
    }

    /// Sessions from `login` expire after this duration.
    pub fn session_ttl(mut self, ttl: Duration) -> Self {
        self.script.session_ttl = Some(ttl);
        self
    }

    /// The next login fails with this error.
    pub fn fail_login_once(mut self, error: ConnectorError) -> Self {
        Failure::push_once(&mut self.script.login_failure, error);
        self
    }

    /// Every login fails with this error until cleared.
    pub fn fail_login_always(mut self, error: ConnectorError) -> Self {
        self.script.login_failure = Some(Failure::Always(error));
        self
    }

    /// The next fetch of the VIN fails with this error.
    pub fn fail_fetch_once(mut self, vin: Vin, error: ConnectorError) -> Self {
        Failure::push_once(self.script.fetch_failures.entry(vin).or_default(), error);
        self
    }

    /// Finishes the build.
    pub fn build(self) -> (FakeConnector, FakeHandle) {
        let script = Arc::new(Mutex::new(self.script));
        (
            FakeConnector {
                info: self.info,
                script: Arc::clone(&script),
            },
            FakeHandle { script },
        )
    }
}

impl FakeHandle {
    fn script(&self) -> MutexGuard<'_, Script> {
        self.script
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Replaces the vehicle list discovery returns.
    pub fn set_vehicles(&self, vehicles: Vec<DiscoveredVehicle>) {
        self.script().vehicles = vehicles;
    }

    /// Replaces the state a fetch returns for the VIN.
    pub fn set_state(&self, vin: Vin, state: VehicleState) {
        self.script().states.insert(vin, state);
    }

    /// The next login fails with this error.
    pub fn fail_login_once(&self, error: ConnectorError) {
        Failure::push_once(&mut self.script().login_failure, error);
    }

    /// Every login fails with this error until [`clear_login_failure`](Self::clear_login_failure).
    pub fn fail_login_always(&self, error: ConnectorError) {
        self.script().login_failure = Some(Failure::Always(error));
    }

    /// Logins succeed again.
    pub fn clear_login_failure(&self) {
        self.script().login_failure = None;
    }

    /// The next refresh fails with this error.
    pub fn fail_refresh_once(&self, error: ConnectorError) {
        Failure::push_once(&mut self.script().refresh_failure, error);
    }

    /// The next vehicle listing fails with this error.
    pub fn fail_list_once(&self, error: ConnectorError) {
        Failure::push_once(&mut self.script().list_failure, error);
    }

    /// The next fetch of the VIN fails with this error. Call repeatedly to queue several.
    pub fn fail_fetch_once(&self, vin: Vin, error: ConnectorError) {
        Failure::push_once(self.script().fetch_failures.entry(vin).or_default(), error);
    }

    /// Every fetch of the VIN fails with this error until [`clear_fetch_failure`](Self::clear_fetch_failure).
    pub fn fail_fetch_always(&self, vin: Vin, error: ConnectorError) {
        self.script()
            .fetch_failures
            .insert(vin, Some(Failure::Always(error)));
    }

    /// Fetches of the VIN succeed again.
    pub fn clear_fetch_failure(&self, vin: &Vin) {
        self.script().fetch_failures.remove(vin);
    }

    /// The next fetch of the VIN panics, to test task isolation.
    pub fn panic_on_next_fetch(&self, vin: Vin) {
        self.script().panic_on_fetch = Some(vin);
    }

    /// A copy of the counters.
    pub fn counters(&self) -> Counters {
        self.script().counters.clone()
    }

    /// When each fetch happened, in call order.
    pub fn fetch_times(&self) -> Vec<(Vin, Instant)> {
        self.script().fetch_times.clone()
    }
}

#[async_trait]
impl Connector for FakeConnector {
    fn info(&self) -> &ConnectorInfo {
        &self.info
    }

    async fn login(&self, _credentials: &Credentials) -> Result<SessionState, ConnectorError> {
        let mut script = self.script();
        script.counters.logins += 1;
        if let Some(error) = Failure::take(&mut script.login_failure) {
            return Err(error);
        }
        script.login_sequence += 1;
        let sequence = script.login_sequence;
        let expires_at = script
            .session_ttl
            .and_then(|ttl| chrono::Duration::from_std(ttl).ok())
            .map(|ttl| Utc::now() + ttl);
        Ok(SessionState {
            brand: self.info.brand,
            payload: serde_json::json!({ "fake_token": format!("token-{sequence}") }),
            expires_at,
        })
    }

    async fn refresh(&self, session: &mut SessionState) -> Result<(), ConnectorError> {
        let mut script = self.script();
        script.counters.refreshes += 1;
        if let Some(error) = Failure::take(&mut script.refresh_failure) {
            return Err(error);
        }
        if let Some(ttl) = script
            .session_ttl
            .and_then(|ttl| chrono::Duration::from_std(ttl).ok())
        {
            session.expires_at = Some(Utc::now() + ttl);
        }
        Ok(())
    }

    async fn list_vehicles(
        &self,
        _session: &mut SessionState,
    ) -> Result<Vec<DiscoveredVehicle>, ConnectorError> {
        let mut script = self.script();
        script.counters.lists += 1;
        if let Some(error) = Failure::take(&mut script.list_failure) {
            return Err(error);
        }
        Ok(script.vehicles.clone())
    }

    async fn fetch_state(
        &self,
        _session: &mut SessionState,
        vin: &Vin,
    ) -> Result<VehicleState, ConnectorError> {
        let mut script = self.script();
        *script.counters.fetches.entry(vin.clone()).or_default() += 1;
        script.fetch_times.push((vin.clone(), Instant::now()));
        if script.panic_on_fetch.as_ref() == Some(vin) {
            script.panic_on_fetch = None;
            drop(script);
            panic!("scripted panic in FakeConnector::fetch_state");
        }
        if let Some(slot) = script.fetch_failures.get_mut(vin)
            && let Some(error) = Failure::take(slot)
        {
            return Err(error);
        }
        match script.states.get(vin) {
            Some(state) => {
                let mut state = state.clone();
                state.fetched_at = Utc::now();
                Ok(state)
            }
            None => Err(ConnectorError::VehicleNotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use vag2mqtt_domain::{Drivetrain, Secret};

    use super::*;

    fn vin() -> Vin {
        Vin::new("WAUZZZ0000000TEST").unwrap()
    }

    fn credentials() -> Credentials {
        Credentials {
            username: "driver@example.test".into(),
            password: Secret::new("pw".into()),
        }
    }

    fn discovered() -> DiscoveredVehicle {
        DiscoveredVehicle {
            vin: vin(),
            model: Some("A6 e-tron".into()),
            drivetrain: Drivetrain::Electric,
        }
    }

    #[tokio::test]
    async fn scripted_login_failure_then_success() {
        let (connector, handle) = FakeConnector::builder(Brand::Audi)
            .fail_login_once(ConnectorError::InvalidCredentials)
            .build();
        let first = connector.login(&credentials()).await;
        assert_eq!(first.unwrap_err(), ConnectorError::InvalidCredentials);
        let second = connector.login(&credentials()).await.unwrap();
        assert_eq!(second.brand, Brand::Audi);
        assert_eq!(handle.counters().logins, 2);
    }

    #[tokio::test]
    async fn scripted_vehicles_and_states_with_counters() {
        let fetched_at = Utc::now();
        let (connector, handle) = FakeConnector::builder(Brand::Audi)
            .vehicles(vec![discovered()])
            .state(vin(), VehicleState::unsupported(fetched_at))
            .build();
        let mut session = connector.login(&credentials()).await.unwrap();
        let vehicles = connector.list_vehicles(&mut session).await.unwrap();
        assert_eq!(vehicles, vec![discovered()]);
        let state = connector.fetch_state(&mut session, &vin()).await.unwrap();
        assert!(state.fetched_at >= fetched_at);
        let unknown = Vin::new("WAUZZZ0000000NEXT").unwrap();
        assert_eq!(
            connector
                .fetch_state(&mut session, &unknown)
                .await
                .unwrap_err(),
            ConnectorError::VehicleNotFound
        );
        let counters = handle.counters();
        assert_eq!(counters.lists, 1);
        assert_eq!(counters.fetches_of(&vin()), 1);
        assert_eq!(counters.fetches_of(&unknown), 1);
        assert_eq!(handle.fetch_times().len(), 2);
    }

    #[tokio::test]
    async fn handle_changes_the_script_while_in_use() {
        let (connector, handle) = FakeConnector::builder(Brand::Audi)
            .state(vin(), VehicleState::unsupported(Utc::now()))
            .build();
        let mut session = connector.login(&credentials()).await.unwrap();
        handle.fail_fetch_once(vin(), ConnectorError::SessionExpired);
        handle.fail_fetch_once(
            vin(),
            ConnectorError::RateLimited {
                retry_after: Some(Duration::from_secs(1)),
            },
        );
        assert_eq!(
            connector
                .fetch_state(&mut session, &vin())
                .await
                .unwrap_err(),
            ConnectorError::SessionExpired
        );
        assert!(matches!(
            connector
                .fetch_state(&mut session, &vin())
                .await
                .unwrap_err(),
            ConnectorError::RateLimited { .. }
        ));
        assert!(connector.fetch_state(&mut session, &vin()).await.is_ok());

        handle.fail_login_always(ConnectorError::TwoFactorRequired);
        assert!(connector.login(&credentials()).await.is_err());
        assert!(connector.login(&credentials()).await.is_err());
        handle.clear_login_failure();
        assert!(connector.login(&credentials()).await.is_ok());
    }

    #[tokio::test]
    async fn session_ttl_sets_expiry_and_refresh_extends_it() {
        let (connector, _handle) = FakeConnector::builder(Brand::Audi)
            .session_ttl(Duration::from_secs(3600))
            .build();
        let mut session = connector.login(&credentials()).await.unwrap();
        let first = session.expires_at.unwrap();
        assert!(first > Utc::now());
        connector.refresh(&mut session).await.unwrap();
        assert!(session.expires_at.unwrap() >= first);
    }
}
