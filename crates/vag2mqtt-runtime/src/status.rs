//! The status snapshot the management API reads.

use chrono::{DateTime, Utc};
use vag2mqtt_domain::{Account, MqttConfig, Vehicle, VehicleState};

/// Everything the UI shows about the running service, refreshed by the supervisor after every
/// command, task event and stale check.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeStatus {
    /// When the supervisor started.
    pub started_at: DateTime<Utc>,
    /// When this snapshot was taken.
    pub snapshot_at: DateTime<Utc>,
    /// The MQTT connection.
    pub mqtt: MqttStatus,
    /// Every account.
    pub accounts: Vec<AccountStatus>,
    /// Every vehicle.
    pub vehicles: Vec<VehicleStatus>,
}

/// The MQTT connection as the supervisor sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MqttStatus {
    /// A configuration exists.
    pub configured: bool,
    /// The configuration is enabled.
    pub enabled: bool,
    /// The publisher has a live broker session.
    pub connected: bool,
    /// The stored configuration, so the management API can show it without reading the database.
    ///
    /// Carries no password: [`MqttConfig`] has no such field, the password lives encrypted in the
    /// secrets table and never leaves it except towards the broker.
    pub config: Option<MqttConfig>,
}

/// An account with the runtime's in-memory extras.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountStatus {
    /// The account record as persisted, including its connection state and last error.
    pub account: Account,
    /// When the next poll is due, if the task is running.
    pub next_poll_at: Option<DateTime<Utc>>,
    /// Consecutive transient failures.
    pub consecutive_failures: u32,
    /// How often the task was respawned after a panic since start.
    pub respawns: u32,
    /// Whether an account task is running.
    pub running: bool,
}

/// A vehicle with its last snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct VehicleStatus {
    /// The vehicle record as persisted.
    pub vehicle: Vehicle,
    /// The last snapshot, if any was ever fetched.
    pub last_state: Option<VehicleState>,
}
