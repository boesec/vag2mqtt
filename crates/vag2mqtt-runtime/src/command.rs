//! Commands from the management layer into the supervisor.

use tokio::sync::{mpsc, oneshot, watch};
use vag2mqtt_domain::{AccountId, Brand, MqttConfig, PollingConfig, Secret, Vin};

use crate::error::RuntimeError;
use crate::status::RuntimeStatus;

/// Input for creating an account (FR-004).
#[derive(Clone, Debug)]
pub struct AccountCreate {
    /// The brand; a connector must be registered for it.
    pub brand: Brand,
    /// The login identifier.
    pub username: String,
    /// The password.
    pub password: Secret<String>,
    /// Polling; `None` uses the runtime default.
    pub polling: Option<PollingConfig>,
    /// Start polling right away.
    pub enabled: bool,
}

/// Changes to an existing account. `None` keeps the current value.
#[derive(Clone, Debug, Default)]
pub struct AccountUpdate {
    /// A new login identifier.
    pub username: Option<String>,
    /// A new polling configuration.
    pub polling: Option<PollingConfig>,
}

pub(crate) enum Command {
    AccountCreate(AccountCreate),
    AccountUpdate(AccountId, AccountUpdate),
    AccountUpdateCredentials(AccountId, Secret<String>),
    AccountSetEnabled(AccountId, bool),
    AccountReauthenticate(AccountId),
    AccountDelete(AccountId),
    VehicleSetEnabled(Vin, bool),
    VehicleSetDisplayName(Vin, Option<String>),
    VehicleRefreshNow(Vin),
    VehicleDelete(Vin),
    MqttConfigure(MqttConfig, Option<Secret<String>>),
    MqttSetEnabled(bool),
    Shutdown,
}

pub(crate) enum Reply {
    Unit,
    AccountId(AccountId),
}

pub(crate) struct Envelope {
    pub(crate) command: Command,
    pub(crate) reply: oneshot::Sender<Result<Reply, RuntimeError>>,
}

/// The management layer's handle on the runtime. Cheap to clone.
#[derive(Clone)]
pub struct RuntimeHandle {
    pub(crate) tx: mpsc::Sender<Envelope>,
    pub(crate) status: watch::Receiver<RuntimeStatus>,
}

impl std::fmt::Debug for RuntimeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RuntimeHandle")
    }
}

impl RuntimeHandle {
    async fn send(&self, command: Command) -> Result<Reply, RuntimeError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Envelope {
                command,
                reply: reply_tx,
            })
            .await
            .map_err(|_| RuntimeError::Stopped)?;
        reply_rx.await.map_err(|_| RuntimeError::Stopped)?
    }

    async fn send_unit(&self, command: Command) -> Result<(), RuntimeError> {
        self.send(command).await.map(|_| ())
    }

    /// The latest status snapshot. Keep the receiver to be notified of changes.
    pub fn status(&self) -> watch::Receiver<RuntimeStatus> {
        self.status.clone()
    }

    /// The current snapshot.
    pub fn current_status(&self) -> RuntimeStatus {
        self.status.borrow().clone()
    }

    /// Creates an account and starts the FR-004 flow. Replies once the rows are committed.
    pub async fn create_account(&self, input: AccountCreate) -> Result<AccountId, RuntimeError> {
        match self.send(Command::AccountCreate(input)).await? {
            Reply::AccountId(id) => Ok(id),
            Reply::Unit => Err(RuntimeError::Stopped),
        }
    }

    /// Changes user name or polling. A running task picks the new interval up immediately.
    pub async fn update_account(
        &self,
        id: &AccountId,
        update: AccountUpdate,
    ) -> Result<(), RuntimeError> {
        self.send_unit(Command::AccountUpdate(id.clone(), update))
            .await
    }

    /// Stores a new password, forgets the session and triggers a new login (ER-005).
    pub async fn update_credentials(
        &self,
        id: &AccountId,
        password: Secret<String>,
    ) -> Result<(), RuntimeError> {
        self.send_unit(Command::AccountUpdateCredentials(id.clone(), password))
            .await
    }

    /// Enables or disables the account.
    pub async fn set_account_enabled(
        &self,
        id: &AccountId,
        enabled: bool,
    ) -> Result<(), RuntimeError> {
        self.send_unit(Command::AccountSetEnabled(id.clone(), enabled))
            .await
    }

    /// The manual retry after an authentication error (ER-005).
    pub async fn reauthenticate(&self, id: &AccountId) -> Result<(), RuntimeError> {
        self.send_unit(Command::AccountReauthenticate(id.clone()))
            .await
    }

    /// Stops the task, clears the retained topics and deletes the rows.
    pub async fn delete_account(&self, id: &AccountId) -> Result<(), RuntimeError> {
        self.send_unit(Command::AccountDelete(id.clone())).await
    }

    /// Enables or disables a vehicle.
    pub async fn set_vehicle_enabled(&self, vin: &Vin, enabled: bool) -> Result<(), RuntimeError> {
        self.send_unit(Command::VehicleSetEnabled(vin.clone(), enabled))
            .await
    }

    /// Sets or clears the display name and republishes the vehicle's `full` payload.
    pub async fn set_vehicle_display_name(
        &self,
        vin: &Vin,
        name: Option<String>,
    ) -> Result<(), RuntimeError> {
        self.send_unit(Command::VehicleSetDisplayName(vin.clone(), name))
            .await
    }

    /// Polls the vehicle once, now, without moving its schedule.
    pub async fn refresh_vehicle_now(&self, vin: &Vin) -> Result<(), RuntimeError> {
        self.send_unit(Command::VehicleRefreshNow(vin.clone()))
            .await
    }

    /// Removes the vehicle explicitly (FR-005) and clears its retained topics.
    pub async fn delete_vehicle(&self, vin: &Vin) -> Result<(), RuntimeError> {
        self.send_unit(Command::VehicleDelete(vin.clone())).await
    }

    /// Stores the broker configuration and reconnects the publisher.
    pub async fn configure_mqtt(
        &self,
        config: MqttConfig,
        password: Option<Secret<String>>,
    ) -> Result<(), RuntimeError> {
        self.send_unit(Command::MqttConfigure(config, password))
            .await
    }

    /// Enables or disables publishing without touching the rest of the configuration.
    pub async fn set_mqtt_enabled(&self, enabled: bool) -> Result<(), RuntimeError> {
        self.send_unit(Command::MqttSetEnabled(enabled)).await
    }

    /// Stops every account task and the publisher.
    pub async fn shutdown(&self) -> Result<(), RuntimeError> {
        self.send_unit(Command::Shutdown).await
    }
}
