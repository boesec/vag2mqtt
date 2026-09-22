//! The supervisor: owns account tasks, the publisher and the status snapshot.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{Id, JoinHandle, JoinSet};
use tokio::time::Instant;
use vag2mqtt_connector_api::ConnectorRegistry;
use vag2mqtt_domain::{
    Account, AccountConnectionState, AccountId, ErrorCategory, LastError, MqttConfig, Secret,
    VehicleDataState, Vin,
};
use vag2mqtt_persistence::{AccountConfigPatch, Database, NewAccount};

use crate::account_task::{self, AccountCommand, AccountEvent, AccountTaskDeps, StopReason};
use crate::clock::Clock;
use crate::command::{AccountCreate, AccountUpdate, Command, Envelope, Reply, RuntimeHandle};
use crate::error::RuntimeError;
use crate::publisher::{Publisher, PublisherFactory};
use crate::settings::RuntimeSettings;
use crate::status::{AccountStatus, MqttStatus, RuntimeStatus, VehicleStatus};

const LOG: &str = "vag2mqtt::runtime";
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// What the supervisor needs to start.
pub struct RuntimeDeps {
    /// The database.
    pub db: Database,
    /// The compiled-in connectors.
    pub registry: ConnectorRegistry,
    /// Creates a publisher for a broker configuration.
    pub publisher_factory: PublisherFactory,
    /// The wall clock.
    pub clock: Arc<dyn Clock>,
    /// Settings; `None` loads them from the database with defaults.
    pub settings: Option<RuntimeSettings>,
}

/// Starts and stops the runtime.
pub struct Supervisor;

impl Supervisor {
    /// Loads accounts and the broker configuration, spawns everything and returns the handle
    /// and the supervisor task.
    pub async fn start(deps: RuntimeDeps) -> Result<(RuntimeHandle, JoinHandle<()>), RuntimeError> {
        let settings = match deps.settings {
            Some(settings) => settings,
            None => RuntimeSettings::load(&deps.db).await?,
        };
        let started_at = deps.clock.now();
        let (publisher_tx, _) = watch::channel(None);
        let (events_tx, events_rx) = mpsc::channel(256);
        let (respawn_tx, respawn_rx) = mpsc::channel(32);
        let (command_tx, command_rx) = mpsc::channel(64);
        let (status_tx, status_rx) = watch::channel(RuntimeStatus {
            started_at,
            snapshot_at: started_at,
            mqtt: MqttStatus {
                configured: false,
                enabled: false,
                connected: false,
            },
            accounts: Vec::new(),
            vehicles: Vec::new(),
        });

        let mut state = State {
            db: deps.db,
            registry: deps.registry,
            factory: deps.publisher_factory,
            clock: deps.clock,
            settings,
            started_at,
            publisher_tx,
            publisher: None,
            tasks: JoinSet::new(),
            controls: HashMap::new(),
            task_ids: HashMap::new(),
            extras: HashMap::new(),
            events_tx,
            respawn_tx,
            status_tx,
        };
        state.start_publisher_from_db().await?;
        state.spawn_enabled_accounts().await?;
        state.refresh_status().await;

        let task = tokio::spawn(state.run(command_rx, events_rx, respawn_rx));
        Ok((
            RuntimeHandle {
                tx: command_tx,
                status: status_rx,
            },
            task,
        ))
    }
}

struct AccountControl {
    tx: mpsc::Sender<AccountCommand>,
    task_id: Id,
}

#[derive(Default)]
struct Extras {
    next_poll_at: Option<DateTime<Utc>>,
    consecutive_failures: u32,
    respawns: u32,
    respawn_times: VecDeque<Instant>,
}

struct State {
    db: Database,
    registry: ConnectorRegistry,
    factory: PublisherFactory,
    clock: Arc<dyn Clock>,
    settings: RuntimeSettings,
    started_at: DateTime<Utc>,
    publisher_tx: watch::Sender<Option<Arc<dyn Publisher>>>,
    publisher: Option<Arc<dyn Publisher>>,
    tasks: JoinSet<(AccountId, StopReason)>,
    controls: HashMap<AccountId, AccountControl>,
    task_ids: HashMap<Id, AccountId>,
    extras: HashMap<AccountId, Extras>,
    events_tx: mpsc::Sender<AccountEvent>,
    respawn_tx: mpsc::Sender<AccountId>,
    status_tx: watch::Sender<RuntimeStatus>,
}

impl State {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<Envelope>,
        mut events: mpsc::Receiver<AccountEvent>,
        mut respawns: mpsc::Receiver<AccountId>,
    ) {
        let mut ticker = tokio::time::interval(self.settings.stale_check_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                envelope = commands.recv() => {
                    let Some(Envelope { command, reply }) = envelope else {
                        tracing::info!(target: LOG, "all runtime handles dropped, shutting down");
                        self.shutdown().await;
                        break;
                    };
                    let is_shutdown = matches!(command, Command::Shutdown);
                    let result = self.handle(command).await;
                    self.refresh_status().await;
                    let _ = reply.send(result);
                    if is_shutdown {
                        break;
                    }
                }
                Some(event) = events.recv() => {
                    let extras = self.extras.entry(event.id.clone()).or_default();
                    extras.next_poll_at = event.next_poll_at;
                    extras.consecutive_failures = event.consecutive_failures;
                    self.update_operational().await;
                    self.refresh_status().await;
                }
                Some(id) = respawns.recv() => {
                    self.respawn(&id).await;
                    self.refresh_status().await;
                }
                Some(exit) = self.tasks.join_next_with_id(), if !self.tasks.is_empty() => {
                    self.on_task_exit(exit).await;
                    self.refresh_status().await;
                }
                _ = ticker.tick() => {
                    self.stale_check().await;
                    self.update_operational().await;
                    self.refresh_status().await;
                }
            }
        }
    }

    // ----- startup ---------------------------------------------------------------------

    async fn start_publisher_from_db(&mut self) -> Result<(), RuntimeError> {
        let Some(config) = self.db.mqtt().get().await? else {
            tracing::warn!(target: LOG, "no MQTT broker configured; polling continues, nothing is published");
            return Ok(());
        };
        if !config.enabled {
            tracing::info!(target: LOG, "MQTT publishing is disabled");
            return Ok(());
        }
        let password = self.db.secrets().mqtt_password().await?;
        self.replace_publisher(Some((config, password))).await
    }

    async fn replace_publisher(
        &mut self,
        next: Option<(MqttConfig, Option<Secret<String>>)>,
    ) -> Result<(), RuntimeError> {
        if let Some(old) = self.publisher.take() {
            self.publisher_tx.send_replace(None);
            old.shutdown().await;
        }
        if let Some((config, password)) = next {
            let publisher = (self.factory)(&config, password.as_ref())?;
            self.publisher = Some(Arc::clone(&publisher));
            self.publisher_tx.send_replace(Some(publisher));
            tracing::info!(target: LOG, host = %config.host, port = config.port, prefix = %config.topic_prefix, "MQTT publisher started");
        }
        Ok(())
    }

    async fn spawn_enabled_accounts(&mut self) -> Result<(), RuntimeError> {
        for account in self.db.accounts().list().await? {
            if account.enabled {
                if let Err(error) = self.spawn_account(&account) {
                    tracing::error!(target: LOG, account_id = %account.id, %error, "account not started");
                    self.record_account_error(
                        &account.id,
                        ErrorCategory::Configuration,
                        error.to_string(),
                    )
                    .await;
                }
            } else {
                self.db
                    .accounts()
                    .update_runtime_status(
                        &account.id,
                        AccountConnectionState::Disabled,
                        account.last_success_at,
                        account.last_error.as_ref(),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    fn spawn_account(&mut self, account: &Account) -> Result<(), RuntimeError> {
        if self.controls.contains_key(&account.id) {
            return Ok(());
        }
        let connector = self
            .registry
            .get(account.brand)
            .ok_or(RuntimeError::UnknownBrand(account.brand))?;
        let (tx, rx) = mpsc::channel(32);
        let deps = AccountTaskDeps {
            db: self.db.clone(),
            connector,
            publisher: self.publisher_tx.subscribe(),
            settings: self.settings.clone(),
            clock: Arc::clone(&self.clock),
            events: self.events_tx.clone(),
        };
        let id = account.id.clone();
        let task_id_holder = id.clone();
        let abort = self.tasks.spawn(async move {
            let reason = account_task::run(task_id_holder.clone(), deps, rx).await;
            (task_id_holder, reason)
        });
        self.task_ids.insert(abort.id(), id.clone());
        self.controls.insert(
            id.clone(),
            AccountControl {
                tx,
                task_id: abort.id(),
            },
        );
        self.extras.entry(id.clone()).or_default();
        tracing::info!(target: LOG, account_id = %id, "account task started");
        Ok(())
    }

    async fn record_account_error(&self, id: &AccountId, category: ErrorCategory, message: String) {
        let error = LastError {
            at: self.clock.now(),
            category,
            message,
        };
        let _ = self
            .db
            .accounts()
            .update_runtime_status(id, AccountConnectionState::Error, None, Some(&error))
            .await;
    }

    // ----- task lifecycle ----------------------------------------------------------------

    async fn on_task_exit(
        &mut self,
        exit: Result<(Id, (AccountId, StopReason)), tokio::task::JoinError>,
    ) {
        match exit {
            Ok((task_id, (id, reason))) => {
                self.task_ids.remove(&task_id);
                if self.controls.get(&id).is_some_and(|c| c.task_id == task_id) {
                    self.controls.remove(&id);
                }
                tracing::info!(target: LOG, account_id = %id, ?reason, "account task stopped");
            }
            Err(join_error) => {
                let task_id = join_error.id();
                let Some(id) = self.task_ids.remove(&task_id) else {
                    return;
                };
                if self.controls.get(&id).is_some_and(|c| c.task_id == task_id) {
                    self.controls.remove(&id);
                }
                if !join_error.is_panic() {
                    return;
                }
                tracing::error!(target: LOG, account_id = %id, "account task panicked; it will be restarted");
                self.record_account_error(
                    &id,
                    ErrorCategory::Internal,
                    "account task panicked".to_string(),
                )
                .await;
                let extras = self.extras.entry(id.clone()).or_default();
                let now = Instant::now();
                while extras
                    .respawn_times
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(3600))
                {
                    extras.respawn_times.pop_front();
                }
                if extras.respawn_times.len() as u32 >= self.settings.respawns_per_hour {
                    tracing::error!(target: LOG, account_id = %id, "respawn limit reached; the account stays down until a command touches it");
                    return;
                }
                extras.respawn_times.push_back(now);
                extras.respawns += 1;
                let delay = self.settings.respawn_delay;
                let tx = self.respawn_tx.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(delay).await;
                    let _ = tx.send(id).await;
                });
            }
        }
    }

    async fn respawn(&mut self, id: &AccountId) {
        if self.controls.contains_key(id) {
            return;
        }
        match self.db.accounts().get(id).await {
            Ok(Some(account)) if account.enabled => {
                if let Err(error) = self.spawn_account(&account) {
                    tracing::error!(target: LOG, account_id = %id, %error, "respawn failed");
                }
            }
            _ => {}
        }
    }

    async fn stop_account(&mut self, id: &AccountId, reason: StopReason) {
        let Some(control) = self.controls.remove(id) else {
            return;
        };
        self.task_ids.remove(&control.task_id);
        let (ack_tx, ack_rx) = oneshot::channel();
        if control
            .tx
            .send(AccountCommand::Stop(reason, ack_tx))
            .await
            .is_ok()
        {
            let _ = tokio::time::timeout(STOP_TIMEOUT, ack_rx).await;
        }
    }

    async fn send_to_account(&self, id: &AccountId, command: AccountCommand) {
        if let Some(control) = self.controls.get(id) {
            let _ = control.tx.send(command).await;
        }
    }

    // ----- commands ----------------------------------------------------------------------

    async fn handle(&mut self, command: Command) -> Result<Reply, RuntimeError> {
        match command {
            Command::AccountCreate(input) => self.create_account(input).await,
            Command::AccountUpdate(id, update) => {
                self.update_account(&id, update).await?;
                Ok(Reply::Unit)
            }
            Command::AccountUpdateCredentials(id, password) => {
                self.require_account(&id).await?;
                self.db
                    .secrets()
                    .set_account_password(&id, &password)
                    .await?;
                self.db.secrets().delete_account_session(&id).await?;
                if self.controls.contains_key(&id) {
                    self.send_to_account(&id, AccountCommand::CredentialsChanged)
                        .await;
                } else {
                    self.respawn(&id).await;
                }
                Ok(Reply::Unit)
            }
            Command::AccountSetEnabled(id, enabled) => {
                let account = self.require_account(&id).await?;
                self.db.accounts().set_enabled(&id, enabled).await?;
                if enabled {
                    let account = Account {
                        enabled: true,
                        ..account
                    };
                    self.spawn_account(&account)?;
                } else {
                    self.stop_account(&id, StopReason::Disabled).await;
                    self.db
                        .accounts()
                        .update_runtime_status(
                            &id,
                            AccountConnectionState::Disabled,
                            account.last_success_at,
                            account.last_error.as_ref(),
                        )
                        .await?;
                }
                Ok(Reply::Unit)
            }
            Command::AccountReauthenticate(id) => {
                self.require_account(&id).await?;
                if self.controls.contains_key(&id) {
                    self.send_to_account(&id, AccountCommand::Reauthenticate)
                        .await;
                } else {
                    self.respawn(&id).await;
                }
                Ok(Reply::Unit)
            }
            Command::AccountDelete(id) => {
                self.require_account(&id).await?;
                self.stop_account(&id, StopReason::Deleted).await;
                let vehicles = self.db.vehicles().list_for_account(&id).await?;
                if let Some(publisher) = &self.publisher {
                    for vehicle in &vehicles {
                        if let Err(error) = publisher.clear_vehicle(&vehicle.vin) {
                            tracing::warn!(target: LOG, vin = %vehicle.vin.short(), %error, "could not clear retained topics");
                        }
                    }
                    if let Err(error) = publisher.clear_account(&id) {
                        tracing::warn!(target: LOG, account_id = %id, %error, "could not clear the account topic");
                    }
                }
                self.db.accounts().delete(&id).await?;
                self.extras.remove(&id);
                tracing::info!(target: LOG, account_id = %id, vehicles = vehicles.len(), "account deleted");
                Ok(Reply::Unit)
            }
            Command::VehicleSetEnabled(vin, enabled) => {
                let vehicle = self.require_vehicle(&vin).await?;
                self.db.vehicles().set_enabled(&vin, enabled).await?;
                if self.controls.contains_key(&vehicle.account_id) {
                    self.send_to_account(&vehicle.account_id, AccountCommand::VehiclesChanged)
                        .await;
                } else {
                    let state = if enabled {
                        VehicleDataState::Pending
                    } else {
                        VehicleDataState::Disabled
                    };
                    self.db
                        .vehicles()
                        .update_runtime_status(
                            &vin,
                            state,
                            vehicle.last_update_at,
                            vehicle.last_error.as_ref(),
                        )
                        .await?;
                }
                Ok(Reply::Unit)
            }
            Command::VehicleSetDisplayName(vin, name) => {
                let vehicle = self.require_vehicle(&vin).await?;
                self.db
                    .vehicles()
                    .set_display_name(&vin, name.as_deref())
                    .await?;
                self.send_to_account(&vehicle.account_id, AccountCommand::VehiclesChanged)
                    .await;
                Ok(Reply::Unit)
            }
            Command::VehicleRefreshNow(vin) => {
                let vehicle = self.require_vehicle(&vin).await?;
                self.send_to_account(&vehicle.account_id, AccountCommand::RefreshNow(vin))
                    .await;
                Ok(Reply::Unit)
            }
            Command::VehicleDelete(vin) => {
                let vehicle = self.require_vehicle(&vin).await?;
                self.send_to_account(
                    &vehicle.account_id,
                    AccountCommand::VehicleRemoved(vin.clone()),
                )
                .await;
                if let Some(publisher) = &self.publisher
                    && let Err(error) = publisher.clear_vehicle(&vin)
                {
                    tracing::warn!(target: LOG, vin = %vin.short(), %error, "could not clear retained topics");
                }
                self.db.vehicles().delete(&vin).await?;
                tracing::info!(target: LOG, vin = %vin.short(), "vehicle deleted");
                Ok(Reply::Unit)
            }
            Command::MqttConfigure(config, password) => {
                self.db.mqtt().upsert(&config).await?;
                if let Some(password) = &password {
                    self.db.secrets().set_mqtt_password(password).await?;
                }
                let password = match password {
                    Some(p) => Some(p),
                    None => self.db.secrets().mqtt_password().await?,
                };
                let next = config.enabled.then_some((config, password));
                self.replace_publisher(next).await?;
                Ok(Reply::Unit)
            }
            Command::MqttSetEnabled(enabled) => {
                self.db.mqtt().set_enabled(enabled).await?;
                let next = if enabled {
                    let config = self.db.mqtt().get().await?.ok_or(RuntimeError::NotFound {
                        entity: "mqtt connection",
                    })?;
                    let password = self.db.secrets().mqtt_password().await?;
                    Some((config, password))
                } else {
                    None
                };
                self.replace_publisher(next).await?;
                Ok(Reply::Unit)
            }
            Command::Shutdown => {
                self.shutdown().await;
                Ok(Reply::Unit)
            }
        }
    }

    async fn create_account(&mut self, input: AccountCreate) -> Result<Reply, RuntimeError> {
        let connector = self
            .registry
            .get(input.brand)
            .ok_or(RuntimeError::UnknownBrand(input.brand))?;
        let polling = input.polling.unwrap_or(vag2mqtt_domain::PollingConfig {
            interval: self.settings.default_polling_interval,
        });
        let minimum = connector.info().min_polling_interval;
        if polling.interval < minimum {
            return Err(RuntimeError::IntervalBelowMinimum { minimum });
        }
        if input.username.trim().is_empty() {
            return Err(RuntimeError::Invalid {
                reason: "username is empty".to_string(),
            });
        }
        let account = self
            .db
            .accounts()
            .insert(
                NewAccount {
                    brand: input.brand,
                    username: input.username,
                    enabled: input.enabled,
                    polling,
                },
                &input.password,
            )
            .await?;
        if account.enabled {
            self.spawn_account(&account)?;
        } else {
            self.db
                .accounts()
                .update_runtime_status(&account.id, AccountConnectionState::Disabled, None, None)
                .await?;
        }
        tracing::info!(target: LOG, account_id = %account.id, brand = %account.brand, "account created");
        Ok(Reply::AccountId(account.id))
    }

    async fn update_account(
        &mut self,
        id: &AccountId,
        update: AccountUpdate,
    ) -> Result<(), RuntimeError> {
        let account = self.require_account(id).await?;
        if let Some(polling) = &update.polling {
            let connector = self
                .registry
                .get(account.brand)
                .ok_or(RuntimeError::UnknownBrand(account.brand))?;
            let minimum = connector.info().min_polling_interval;
            if polling.interval < minimum {
                return Err(RuntimeError::IntervalBelowMinimum { minimum });
            }
        }
        if let Some(username) = &update.username
            && username.trim().is_empty()
        {
            return Err(RuntimeError::Invalid {
                reason: "username is empty".to_string(),
            });
        }
        self.db
            .accounts()
            .update_config(
                id,
                AccountConfigPatch {
                    username: update.username,
                    polling: update.polling,
                },
            )
            .await?;
        self.send_to_account(id, AccountCommand::ConfigChanged)
            .await;
        Ok(())
    }

    async fn require_account(&self, id: &AccountId) -> Result<Account, RuntimeError> {
        self.db
            .accounts()
            .get(id)
            .await?
            .ok_or(RuntimeError::NotFound { entity: "account" })
    }

    async fn require_vehicle(&self, vin: &Vin) -> Result<vag2mqtt_domain::Vehicle, RuntimeError> {
        self.db
            .vehicles()
            .get(vin)
            .await?
            .ok_or(RuntimeError::NotFound { entity: "vehicle" })
    }

    async fn shutdown(&mut self) {
        let ids: Vec<AccountId> = self.controls.keys().cloned().collect();
        for id in ids {
            self.stop_account(&id, StopReason::Shutdown).await;
        }
        while let Some(exit) = self.tasks.join_next_with_id().await {
            if let Ok((task_id, _)) = exit {
                self.task_ids.remove(&task_id);
            }
        }
        if let Some(publisher) = self.publisher.take() {
            self.publisher_tx.send_replace(None);
            publisher.shutdown().await;
        }
        tracing::info!(target: LOG, "runtime stopped");
    }

    // ----- periodic work -----------------------------------------------------------------

    async fn stale_check(&mut self) {
        let now = self.clock.now();
        let Ok(accounts) = self.db.accounts().list().await else {
            return;
        };
        let Ok(vehicles) = self.db.vehicles().list().await else {
            return;
        };
        let intervals: HashMap<AccountId, Duration> = accounts
            .iter()
            .map(|a| (a.id.clone(), a.polling.interval))
            .collect();
        for vehicle in vehicles {
            if vehicle.data_state != VehicleDataState::Fresh {
                continue;
            }
            let Some(last) = vehicle.last_update_at else {
                continue;
            };
            let interval = intervals
                .get(&vehicle.account_id)
                .copied()
                .unwrap_or(self.settings.default_polling_interval);
            let limit = interval.saturating_mul(self.settings.stale_multiplier.max(1));
            let age = (now - last).to_std().unwrap_or_default();
            if age > limit {
                tracing::warn!(target: LOG, vin = %vehicle.vin.short(), age_secs = age.as_secs(), "vehicle data is stale");
                let _ = self
                    .db
                    .vehicles()
                    .update_runtime_status(
                        &vehicle.vin,
                        VehicleDataState::Stale,
                        vehicle.last_update_at,
                        vehicle.last_error.as_ref(),
                    )
                    .await;
                if let Some(publisher) = &self.publisher {
                    let _ = publisher.publish_vehicle_availability(
                        &vehicle.vin,
                        VehicleDataState::Stale,
                        now,
                    );
                }
            }
        }
    }

    async fn update_operational(&self) {
        let Some(publisher) = &self.publisher else {
            return;
        };
        let any_fresh = self
            .db
            .vehicles()
            .list()
            .await
            .map(|v| v.iter().any(|v| v.data_state == VehicleDataState::Fresh))
            .unwrap_or(false);
        let _ = publisher.set_operational(any_fresh);
    }

    async fn refresh_status(&self) {
        let now = self.clock.now();
        let accounts = self.db.accounts().list().await.unwrap_or_default();
        let vehicles = self.db.vehicles().list().await.unwrap_or_default();
        let mqtt_config = self.db.mqtt().get().await.ok().flatten();
        let mut vehicle_statuses = Vec::with_capacity(vehicles.len());
        for vehicle in vehicles {
            let last_state = self
                .db
                .vehicle_states()
                .get(&vehicle.vin)
                .await
                .ok()
                .flatten()
                .map(|stored| stored.state);
            vehicle_statuses.push(VehicleStatus {
                vehicle,
                last_state,
            });
        }
        let status = RuntimeStatus {
            started_at: self.started_at,
            snapshot_at: now,
            mqtt: MqttStatus {
                configured: mqtt_config.is_some(),
                enabled: mqtt_config.is_some_and(|c| c.enabled),
                connected: self.publisher.as_ref().is_some_and(|p| p.is_connected()),
            },
            accounts: accounts
                .into_iter()
                .map(|account| {
                    let extras = self.extras.get(&account.id);
                    AccountStatus {
                        running: self.controls.contains_key(&account.id),
                        next_poll_at: extras.and_then(|e| e.next_poll_at),
                        consecutive_failures: extras.map(|e| e.consecutive_failures).unwrap_or(0),
                        respawns: extras.map(|e| e.respawns).unwrap_or(0),
                        account,
                    }
                })
                .collect(),
            vehicles: vehicle_statuses,
        };
        self.status_tx.send_replace(status);
    }
}
