//! One task per enabled account: login, discovery, sequential polling, persistence, publishing.
//!
//! The task owns the account's session and talks to the outside only through messages: commands
//! in, status events out, the publisher through a `watch` channel. No lock is held across an
//! await.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;
use vag2mqtt_connector_api::{Connector, ConnectorError, Credentials, ErrorClass, SessionState};
use vag2mqtt_domain::{
    Account, AccountConnectionState, AccountId, ErrorCategory, LastError, Vehicle,
    VehicleDataState, VehicleState, Vin,
};
use vag2mqtt_mqtt::VehicleMeta;
use vag2mqtt_persistence::Database;

use crate::backoff;
use crate::clock::Clock;
use crate::publisher::Publisher;
use crate::settings::RuntimeSettings;

const LOG_ACCOUNT: &str = "vag2mqtt::account";
const LOG_VEHICLE: &str = "vag2mqtt::vehicle";

/// Why a task stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StopReason {
    Disabled,
    Deleted,
    Shutdown,
}

/// Commands from the supervisor to a running account task.
pub(crate) enum AccountCommand {
    /// Reload the account row (interval, user name).
    ConfigChanged,
    /// Reload the password, forget the session, clear the auth error and poll now.
    CredentialsChanged,
    /// Forget the session, clear the auth error and poll now.
    Reauthenticate,
    /// Reload the vehicle rows (enabled flags, names) and republish what changed.
    VehiclesChanged,
    /// Poll one vehicle now without moving the schedule.
    RefreshNow(Vin),
    /// The supervisor removed the vehicle; forget it.
    VehicleRemoved(Vin),
    /// Stop, acknowledging when done.
    Stop(StopReason, oneshot::Sender<()>),
}

/// What the task tells the supervisor after every cycle, for the status snapshot.
#[derive(Clone, Debug)]
pub(crate) struct AccountEvent {
    pub(crate) id: AccountId,
    pub(crate) next_poll_at: Option<DateTime<Utc>>,
    pub(crate) consecutive_failures: u32,
}

/// What a task needs from the supervisor.
pub(crate) struct AccountTaskDeps {
    pub(crate) db: Database,
    pub(crate) connector: Arc<dyn Connector>,
    pub(crate) publisher: watch::Receiver<Option<Arc<dyn Publisher>>>,
    pub(crate) settings: RuntimeSettings,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) events: mpsc::Sender<AccountEvent>,
}

/// The outcome of one polling cycle at account level.
enum Cycle {
    Success,
    Transient(ConnectorError),
    RateLimited(Option<Duration>),
    AuthError(ConnectorError),
}

/// Why one vehicle poll failed.
enum VehicleFailure {
    /// An account level problem; abort the cycle.
    Account(Cycle),
    /// Retry with backoff.
    Transient(ConnectorError),
    /// Recorded on the vehicle; the cycle continues.
    Permanent,
}

struct Task {
    id: AccountId,
    deps: AccountTaskDeps,
    account: Account,
    credentials: Credentials,
    session: Option<SessionState>,
    vehicles: Vec<Vehicle>,
    vehicle_failures: HashMap<Vin, u32>,
    auth_failed: bool,
    consecutive_failures: u32,
    next_poll_at: Option<Instant>,
    next_poll_wall: Option<DateTime<Utc>>,
    last_discovery: Option<Instant>,
    refresh_queue: VecDeque<Vin>,
}

/// Runs the account until told to stop. Errors inside are handled and reported; only a panic
/// escapes, and the supervisor respawns the task then.
pub(crate) async fn run(
    id: AccountId,
    deps: AccountTaskDeps,
    mut rx: mpsc::Receiver<AccountCommand>,
) -> StopReason {
    let span = tracing::info_span!("account", account_id = %id);
    let _guard = span.enter();

    let mut task = match Task::load(id.clone(), deps).await {
        Ok(task) => task,
        Err(error) => {
            tracing::error!(target: LOG_ACCOUNT, %error, "account task could not start; waiting for a command");
            // Wait for a stop so the supervisor's bookkeeping stays consistent.
            loop {
                match rx.recv().await {
                    Some(AccountCommand::Stop(reason, ack)) => {
                        let _ = ack.send(());
                        return reason;
                    }
                    Some(_) => {}
                    None => return StopReason::Shutdown,
                }
            }
        }
    };
    drop(_guard);

    task.republish_all().await;
    task.report().await;

    loop {
        let delay = task.next_poll_at;
        let idle = task.auth_failed;
        let has_refresh = !task.refresh_queue.is_empty();
        tokio::select! {
            command = rx.recv() => match command {
                None => return StopReason::Shutdown,
                Some(command) => {
                    if let Some(reason) = task.handle(command).await {
                        return reason;
                    }
                }
            },
            changed = task.deps.publisher.changed() => {
                if changed.is_ok() {
                    tracing::info!(target: LOG_ACCOUNT, account_id = %task.id, "publisher changed, republishing");
                    task.republish_all().await;
                }
            },
            _ = async {
                if has_refresh {
                    return;
                }
                if idle {
                    std::future::pending::<()>().await;
                }
                if let Some(at) = delay {
                    tokio::time::sleep_until(at).await;
                }
            } => {
                if let Some(vin) = task.refresh_queue.pop_front() {
                    task.refresh_one(&vin).await;
                } else {
                    task.cycle().await;
                }
            }
        }
    }
}

impl Task {
    async fn load(id: AccountId, deps: AccountTaskDeps) -> Result<Self, String> {
        let account = deps
            .db
            .accounts()
            .get(&id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "account not found".to_string())?;
        let password = deps
            .db
            .secrets()
            .account_password(&id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "account has no password".to_string())?;
        let vehicles = deps
            .db
            .vehicles()
            .list_for_account(&id)
            .await
            .map_err(|e| e.to_string())?;
        let credentials = Credentials {
            username: account.username.clone(),
            password,
        };
        Ok(Self {
            id,
            deps,
            account,
            credentials,
            session: None,
            vehicles,
            vehicle_failures: HashMap::new(),
            auth_failed: false,
            consecutive_failures: 0,
            next_poll_at: None,
            next_poll_wall: None,
            last_discovery: None,
            refresh_queue: VecDeque::new(),
        })
    }

    fn now(&self) -> DateTime<Utc> {
        self.deps.clock.now()
    }

    fn interval(&self) -> Duration {
        self.account
            .polling
            .interval
            .max(self.deps.connector.info().min_polling_interval)
    }

    fn publisher(&self) -> Option<Arc<dyn Publisher>> {
        self.deps.publisher.borrow().clone()
    }

    fn meta(vehicle: &Vehicle) -> VehicleMeta {
        VehicleMeta {
            vin: vehicle.vin.clone(),
            brand: vehicle.brand,
            model: vehicle.model.clone(),
            display_name: vehicle.display_name.clone(),
        }
    }

    async fn report(&self) {
        let _ = self
            .deps
            .events
            .send(AccountEvent {
                id: self.id.clone(),
                next_poll_at: self.next_poll_wall,
                consecutive_failures: self.consecutive_failures,
            })
            .await;
    }

    async fn handle(&mut self, command: AccountCommand) -> Option<StopReason> {
        match command {
            AccountCommand::ConfigChanged => {
                if let Ok(Some(account)) = self.deps.db.accounts().get(&self.id).await {
                    let interval_changed = account.polling != self.account.polling;
                    self.credentials.username = account.username.clone();
                    self.account.polling = account.polling.clone();
                    self.account.username = account.username;
                    if interval_changed && !self.auth_failed {
                        self.schedule(self.interval());
                        tracing::info!(target: LOG_ACCOUNT, account_id = %self.id, interval_secs = self.interval().as_secs(), "polling interval changed");
                    }
                }
                self.report().await;
            }
            AccountCommand::CredentialsChanged => {
                if let Ok(Some(password)) = self.deps.db.secrets().account_password(&self.id).await
                {
                    self.credentials.password = password;
                }
                self.forget_session().await;
                self.auth_failed = false;
                self.consecutive_failures = 0;
                self.poll_soon();
                self.report().await;
            }
            AccountCommand::Reauthenticate => {
                self.forget_session().await;
                self.auth_failed = false;
                self.consecutive_failures = 0;
                self.poll_soon();
                self.report().await;
            }
            AccountCommand::VehiclesChanged => {
                self.reload_vehicles().await;
            }
            AccountCommand::RefreshNow(vin) => {
                if !self.auth_failed {
                    self.refresh_queue.push_back(vin);
                }
            }
            AccountCommand::VehicleRemoved(vin) => {
                self.vehicles.retain(|v| v.vin != vin);
                self.vehicle_failures.remove(&vin);
            }
            AccountCommand::Stop(reason, ack) => {
                if reason == StopReason::Disabled {
                    self.mark_disabled().await;
                }
                let _ = ack.send(());
                return Some(reason);
            }
        }
        None
    }

    fn poll_soon(&mut self) {
        self.next_poll_at = Some(Instant::now());
        self.next_poll_wall = Some(self.now());
    }

    fn schedule(&mut self, delay: Duration) {
        self.next_poll_at = Some(Instant::now() + delay);
        self.next_poll_wall =
            Some(self.now() + chrono::Duration::from_std(delay).unwrap_or_default());
    }

    async fn forget_session(&mut self) {
        self.session = None;
        if let Err(error) = self
            .deps
            .db
            .secrets()
            .delete_account_session(&self.id)
            .await
        {
            tracing::warn!(target: LOG_ACCOUNT, account_id = %self.id, %error, "could not delete the stored session");
        }
    }

    async fn persist_session(&self) {
        let Some(session) = &self.session else {
            return;
        };
        match session.to_bytes() {
            Ok(bytes) => {
                if let Err(error) = self
                    .deps
                    .db
                    .secrets()
                    .set_account_session(&self.id, &bytes)
                    .await
                {
                    tracing::warn!(target: LOG_ACCOUNT, account_id = %self.id, %error, "could not store the session");
                }
            }
            Err(error) => {
                tracing::warn!(target: LOG_ACCOUNT, account_id = %self.id, %error, "could not serialise the session");
            }
        }
    }

    async fn reload_vehicles(&mut self) {
        let Ok(vehicles) = self.deps.db.vehicles().list_for_account(&self.id).await else {
            return;
        };
        let now = self.now();
        let publisher = self.publisher();
        for vehicle in &vehicles {
            let previous = self.vehicles.iter().find(|v| v.vin == vehicle.vin);
            let was_enabled = previous.is_none_or(|p| p.enabled);
            if !vehicle.enabled && was_enabled {
                self.set_vehicle_state(&vehicle.vin, VehicleDataState::Disabled, None, None)
                    .await;
            } else if vehicle.enabled && !was_enabled {
                self.set_vehicle_state(&vehicle.vin, VehicleDataState::Pending, None, None)
                    .await;
                if !self.auth_failed {
                    self.refresh_queue.push_back(vehicle.vin.clone());
                }
            } else if let Some(p) = &publisher
                && previous.is_some_and(|p| {
                    p.display_name != vehicle.display_name || p.model != vehicle.model
                })
                && let Ok(Some(stored)) = self.deps.db.vehicle_states().get(&vehicle.vin).await
            {
                let _ = p.publish_state(&Self::meta(vehicle), &stored.state);
                let _ = p.publish_vehicle_availability(&vehicle.vin, vehicle.data_state, now);
            }
        }
        self.vehicles = self
            .deps
            .db
            .vehicles()
            .list_for_account(&self.id)
            .await
            .unwrap_or(vehicles);
    }

    async fn mark_disabled(&mut self) {
        let now = self.now();
        for vin in self
            .vehicles
            .iter()
            .map(|v| v.vin.clone())
            .collect::<Vec<_>>()
        {
            self.set_vehicle_state(&vin, VehicleDataState::Disabled, None, None)
                .await;
        }
        self.set_account_state(AccountConnectionState::Disabled, None, None)
            .await;
        if let Some(p) = self.publisher() {
            let _ = p.publish_account_availability(&self.id, AccountConnectionState::Disabled, now);
        }
    }

    /// Publishes the last known snapshot and availability of every vehicle, seeding `lc` from
    /// the stored map first. Used at start and whenever the publisher changes.
    async fn republish_all(&mut self) {
        let Some(publisher) = self.publisher() else {
            return;
        };
        let now = self.now();
        for vehicle in self.vehicles.clone() {
            match self.deps.db.vehicle_states().get(&vehicle.vin).await {
                Ok(Some(stored)) => {
                    publisher.seed_last_changes(&vehicle.vin, &stored.last_changes);
                    if let Err(error) =
                        publisher.publish_state(&Self::meta(&vehicle), &stored.state)
                    {
                        tracing::warn!(target: LOG_VEHICLE, vin = %vehicle.vin.short(), %error, "could not republish the stored state");
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(target: LOG_VEHICLE, vin = %vehicle.vin.short(), %error, "could not load the stored state");
                }
            }
            let _ = publisher.publish_vehicle_availability(&vehicle.vin, vehicle.data_state, now);
        }
        let _ =
            publisher.publish_account_availability(&self.id, self.account.connection_state, now);
    }

    async fn set_account_state(
        &mut self,
        state: AccountConnectionState,
        last_success_at: Option<DateTime<Utc>>,
        error: Option<LastError>,
    ) {
        self.account.connection_state = state;
        if last_success_at.is_some() {
            self.account.last_success_at = last_success_at;
        }
        if error.is_some() {
            self.account.last_error = error;
        }
        if let Err(db_error) = self
            .deps
            .db
            .accounts()
            .update_runtime_status(
                &self.id,
                state,
                self.account.last_success_at,
                self.account.last_error.as_ref(),
            )
            .await
        {
            tracing::warn!(target: LOG_ACCOUNT, account_id = %self.id, error = %db_error, "could not store the account status");
        }
        if let Some(p) = self.publisher() {
            let _ = p.publish_account_availability(&self.id, state, self.now());
        }
    }

    async fn set_vehicle_state(
        &mut self,
        vin: &Vin,
        state: VehicleDataState,
        last_update_at: Option<DateTime<Utc>>,
        error: Option<LastError>,
    ) {
        let now = self.now();
        if let Some(vehicle) = self.vehicles.iter_mut().find(|v| &v.vin == vin) {
            vehicle.data_state = state;
            if last_update_at.is_some() {
                vehicle.last_update_at = last_update_at;
            }
            if error.is_some() {
                vehicle.last_error = error;
            }
            if let Err(db_error) = self
                .deps
                .db
                .vehicles()
                .update_runtime_status(
                    vin,
                    state,
                    vehicle.last_update_at,
                    vehicle.last_error.as_ref(),
                )
                .await
            {
                tracing::warn!(target: LOG_VEHICLE, vin = %vin.short(), error = %db_error, "could not store the vehicle status");
            }
        }
        if let Some(p) = self.publisher() {
            let _ = p.publish_vehicle_availability(vin, state, now);
        }
    }

    fn last_error(&self, error: &ConnectorError) -> LastError {
        LastError {
            at: self.now(),
            category: error.category(),
            message: error.to_string(),
        }
    }

    // ----- the polling cycle -------------------------------------------------------------

    async fn cycle(&mut self) {
        let outcome = self.poll_all().await;
        self.apply(outcome, true).await;
        self.report().await;
    }

    async fn refresh_one(&mut self, vin: &Vin) {
        let Some(vehicle) = self.vehicles.iter().find(|v| &v.vin == vin).cloned() else {
            return;
        };
        if !vehicle.enabled || vehicle.missing_since.is_some() {
            return;
        }
        if let Err(cycle) = self.ensure_session().await {
            self.apply(cycle, true).await;
            self.report().await;
            return;
        }
        match self.poll_vehicle(&vehicle).await {
            Ok(()) => {
                self.consecutive_failures = 0;
                let now = self.now();
                self.set_account_state(AccountConnectionState::Ok, Some(now), None)
                    .await;
            }
            Err(VehicleFailure::Account(cycle)) => self.apply(cycle, true).await,
            Err(_) => {}
        }
        self.report().await;
    }

    async fn apply(&mut self, outcome: Cycle, reschedule: bool) {
        let now = self.now();
        match outcome {
            Cycle::Success => {
                self.consecutive_failures = 0;
                self.set_account_state(AccountConnectionState::Ok, Some(now), None)
                    .await;
                if reschedule {
                    self.schedule(self.interval());
                }
            }
            Cycle::Transient(error) => {
                self.consecutive_failures = self.consecutive_failures.saturating_add(1);
                let state = if self.consecutive_failures >= self.deps.settings.failures_to_error {
                    AccountConnectionState::Error
                } else if error.category() == ErrorCategory::Network {
                    AccountConnectionState::Unreachable
                } else {
                    self.account.connection_state
                };
                let last_error = self.last_error(&error);
                tracing::warn!(
                    target: LOG_ACCOUNT,
                    account_id = %self.id,
                    %error,
                    consecutive_failures = self.consecutive_failures,
                    "transient failure, backing off"
                );
                self.set_account_state(state, None, Some(last_error)).await;
                let delay = {
                    let mut rng = rand::rng();
                    backoff::delay(&self.deps.settings, self.consecutive_failures, &mut rng)
                };
                self.schedule(delay);
            }
            Cycle::RateLimited(retry_after) => {
                let pause = retry_after.unwrap_or(self.deps.settings.rate_limit_pause);
                let last_error = LastError {
                    at: now,
                    category: ErrorCategory::RateLimit,
                    message: format!("rate limited, pausing for {} s", pause.as_secs()),
                };
                tracing::warn!(target: LOG_ACCOUNT, account_id = %self.id, pause_secs = pause.as_secs(), "rate limited");
                self.set_account_state(AccountConnectionState::RateLimited, None, Some(last_error))
                    .await;
                self.schedule(pause);
            }
            Cycle::AuthError(error) => {
                let last_error = self.last_error(&error);
                tracing::error!(target: LOG_ACCOUNT, account_id = %self.id, %error, "authentication failed; waiting for new credentials or a manual retry");
                self.auth_failed = true;
                self.next_poll_at = None;
                self.next_poll_wall = None;
                self.refresh_queue.clear();
                self.forget_session().await;
                self.set_account_state(AccountConnectionState::AuthError, None, Some(last_error))
                    .await;
            }
        }
    }

    async fn poll_all(&mut self) -> Cycle {
        if let Err(cycle) = self.ensure_session().await {
            return cycle;
        }
        if self.discovery_due()
            && let Err(cycle) = self.discover().await
        {
            return cycle;
        }
        let targets: Vec<Vehicle> = self
            .vehicles
            .iter()
            .filter(|v| v.enabled && v.missing_since.is_none())
            .cloned()
            .collect();
        let mut any_success = false;
        let mut transient: Option<ConnectorError> = None;
        for vehicle in &targets {
            match self.poll_vehicle(vehicle).await {
                Ok(()) => any_success = true,
                Err(VehicleFailure::Account(cycle)) => return cycle,
                Err(VehicleFailure::Transient(error)) => transient = Some(error),
                Err(VehicleFailure::Permanent) => {}
            }
        }
        match transient {
            Some(error) if !any_success => Cycle::Transient(error),
            _ => Cycle::Success,
        }
    }

    fn discovery_due(&self) -> bool {
        match self.last_discovery {
            None => true,
            Some(at) => at.elapsed() >= self.deps.settings.discovery_interval,
        }
    }

    /// Makes sure `self.session` is usable: stored session, refresh, or a fresh login.
    async fn ensure_session(&mut self) -> Result<(), Cycle> {
        let now = self.now();
        if self.session.is_none()
            && let Ok(Some(bytes)) = self.deps.db.secrets().account_session(&self.id).await
            && let Ok(stored) = SessionState::from_bytes(bytes.expose_secret())
            && stored.brand == self.account.brand
        {
            self.session = Some(stored);
        }
        if let Some(session) = &self.session
            && !session.is_expired_at(now)
        {
            return Ok(());
        }
        if let Some(mut session) = self.session.take() {
            match self.deps.connector.refresh(&mut session).await {
                Ok(()) => {
                    self.session = Some(session);
                    self.persist_session().await;
                    return Ok(());
                }
                Err(error) => {
                    tracing::info!(target: LOG_ACCOUNT, account_id = %self.id, %error, "session refresh failed, logging in again");
                }
            }
        }
        match self.deps.connector.login(&self.credentials).await {
            Ok(session) => {
                tracing::info!(target: LOG_ACCOUNT, account_id = %self.id, "logged in");
                self.session = Some(session);
                self.persist_session().await;
                Ok(())
            }
            Err(error) => Err(match error.class() {
                ErrorClass::AuthInvalid => Cycle::AuthError(error),
                ErrorClass::RateLimited => Cycle::RateLimited(retry_after(&error)),
                _ => Cycle::Transient(error),
            }),
        }
    }

    async fn discover(&mut self) -> Result<(), Cycle> {
        let Some(session) = self.session.as_mut() else {
            return Err(Cycle::Transient(ConnectorError::SessionExpired));
        };
        let discovered = match self.deps.connector.list_vehicles(session).await {
            Ok(list) => list,
            Err(error) => {
                return Err(match error.class() {
                    ErrorClass::AuthInvalid => Cycle::AuthError(error),
                    ErrorClass::RateLimited => Cycle::RateLimited(retry_after(&error)),
                    ErrorClass::SessionExpired => {
                        self.forget_session().await;
                        Cycle::Transient(error)
                    }
                    _ => Cycle::Transient(error),
                });
            }
        };
        self.persist_session().await;
        self.last_discovery = Some(Instant::now());

        let mut seen = HashSet::new();
        for vehicle in &discovered {
            seen.insert(vehicle.vin.clone());
            let input = vag2mqtt_persistence::DiscoveredVehicle {
                vin: vehicle.vin.clone(),
                model: vehicle.model.clone(),
                drivetrain: vehicle.drivetrain,
            };
            match self
                .deps
                .db
                .vehicles()
                .upsert_discovered(&self.id, &input)
                .await
            {
                Ok(record) => {
                    if !self.vehicles.iter().any(|v| v.vin == record.vin) {
                        tracing::info!(target: LOG_VEHICLE, account_id = %self.id, vin = %record.vin.short(), "vehicle discovered");
                    }
                }
                Err(error) => {
                    tracing::warn!(target: LOG_VEHICLE, account_id = %self.id, vin = %vehicle.vin.short(), %error, "discovered vehicle not stored");
                }
            }
        }
        let now = self.now();
        let missing: Vec<Vin> = self
            .vehicles
            .iter()
            .filter(|v| v.missing_since.is_none() && !seen.contains(&v.vin))
            .map(|v| v.vin.clone())
            .collect();
        for vin in missing {
            tracing::warn!(target: LOG_VEHICLE, account_id = %self.id, vin = %vin.short(), "vehicle no longer returned by the manufacturer; kept and marked missing");
            if let Err(error) = self.deps.db.vehicles().mark_missing(&vin, now).await {
                tracing::warn!(target: LOG_VEHICLE, vin = %vin.short(), %error, "could not mark the vehicle missing");
            }
            self.set_vehicle_state(&vin, VehicleDataState::Unavailable, None, None)
                .await;
        }
        if let Ok(vehicles) = self.deps.db.vehicles().list_for_account(&self.id).await {
            self.vehicles = vehicles;
        }
        Ok(())
    }

    async fn poll_vehicle(&mut self, vehicle: &Vehicle) -> Result<(), VehicleFailure> {
        let vin = vehicle.vin.clone();
        match self.fetch_with_retry(&vin).await {
            Ok(state) => {
                self.vehicle_failures.remove(&vin);
                self.on_state(vehicle, state).await;
                Ok(())
            }
            Err(VehicleFailure::Transient(error)) => {
                let failures = self.vehicle_failures.entry(vin.clone()).or_default();
                *failures = failures.saturating_add(1);
                let failures = *failures;
                let last_error = self.last_error(&error);
                let state = if failures >= self.deps.settings.failures_to_error {
                    VehicleDataState::Error
                } else {
                    vehicle.data_state
                };
                tracing::warn!(target: LOG_VEHICLE, vin = %vin.short(), %error, failures, "vehicle fetch failed");
                self.set_vehicle_state(&vin, state, None, Some(last_error))
                    .await;
                Err(VehicleFailure::Transient(error))
            }
            Err(VehicleFailure::Permanent) => Err(VehicleFailure::Permanent),
            Err(VehicleFailure::Account(cycle)) => Err(VehicleFailure::Account(cycle)),
        }
    }

    async fn fetch_with_retry(&mut self, vin: &Vin) -> Result<VehicleState, VehicleFailure> {
        for attempt in 0..2 {
            self.ensure_session()
                .await
                .map_err(VehicleFailure::Account)?;
            let Some(session) = self.session.as_mut() else {
                return Err(VehicleFailure::Transient(ConnectorError::SessionExpired));
            };
            match self.deps.connector.fetch_state(session, vin).await {
                Ok(state) => {
                    self.persist_session().await;
                    return Ok(state);
                }
                Err(error) => match error.class() {
                    ErrorClass::SessionExpired if attempt == 0 => {
                        tracing::info!(target: LOG_ACCOUNT, account_id = %self.id, "session expired, logging in again");
                        self.forget_session().await;
                    }
                    ErrorClass::SessionExpired => {
                        return Err(VehicleFailure::Transient(error));
                    }
                    ErrorClass::AuthInvalid => {
                        return Err(VehicleFailure::Account(Cycle::AuthError(error)));
                    }
                    ErrorClass::RateLimited => {
                        return Err(VehicleFailure::Account(Cycle::RateLimited(retry_after(
                            &error,
                        ))));
                    }
                    ErrorClass::Transient => return Err(VehicleFailure::Transient(error)),
                    ErrorClass::Permanent => {
                        let last_error = self.last_error(&error);
                        tracing::warn!(target: LOG_VEHICLE, vin = %vin.short(), %error, "vehicle fetch failed permanently");
                        self.set_vehicle_state(
                            vin,
                            VehicleDataState::Error,
                            None,
                            Some(last_error),
                        )
                        .await;
                        return Err(VehicleFailure::Permanent);
                    }
                },
            }
        }
        Err(VehicleFailure::Transient(ConnectorError::SessionExpired))
    }

    async fn on_state(&mut self, vehicle: &Vehicle, state: VehicleState) {
        let now = self.now();
        let vin = vehicle.vin.clone();
        let current = self
            .vehicles
            .iter()
            .find(|v| v.vin == vin)
            .cloned()
            .unwrap_or_else(|| vehicle.clone());
        let last_changes = match self.publisher() {
            Some(publisher) => match publisher.publish_state(&Self::meta(&current), &state) {
                Ok(map) => map,
                Err(error) => {
                    tracing::warn!(target: LOG_VEHICLE, vin = %vin.short(), %error, "state not published");
                    self.stored_last_changes(&vin).await
                }
            },
            None => self.stored_last_changes(&vin).await,
        };
        if let Err(error) = self
            .deps
            .db
            .vehicle_states()
            .upsert(&vin, &state, &last_changes)
            .await
        {
            tracing::warn!(target: LOG_VEHICLE, vin = %vin.short(), %error, "could not store the vehicle state");
        }
        tracing::debug!(target: LOG_VEHICLE, vin = %vin.short(), "vehicle state fetched");
        self.set_vehicle_state(&vin, VehicleDataState::Fresh, Some(now), None)
            .await;
    }

    async fn stored_last_changes(&self, vin: &Vin) -> std::collections::BTreeMap<String, i64> {
        self.deps
            .db
            .vehicle_states()
            .get(vin)
            .await
            .ok()
            .flatten()
            .map(|stored| stored.last_changes)
            .unwrap_or_default()
    }
}

fn retry_after(error: &ConnectorError) -> Option<Duration> {
    match error {
        ConnectorError::RateLimited { retry_after } => *retry_after,
        _ => None,
    }
}
