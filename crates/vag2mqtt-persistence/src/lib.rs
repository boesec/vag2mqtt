//! SQLite persistence and secret encryption for VAG2MQTT.
//!
//! One file, `<data-dir>/vag2mqtt.db`, holds accounts, vehicles, the last vehicle state, the
//! MQTT configuration, settings and every secret. Secrets are encrypted with
//! ChaCha20-Poly1305 under a master key that comes from the environment or from
//! `<data-dir>/master.key`, generated on first start.
//!
//! The entry point is [`Database::open`]. Repositories are reached through the accessor methods
//! on [`Database`] (`accounts()`, `vehicles()`, ...). No SQL exists outside this crate.
//!
//! Implemented by WP-02.

mod crypto;
mod database;
mod error;
mod master_key;
mod repos;
mod rows;

pub use database::{DB_FILE_NAME, Database};
pub use error::PersistenceError;
pub use master_key::MasterKeySource;
pub use repos::accounts::{AccountConfigPatch, Accounts, NewAccount};
pub use repos::mqtt::Mqtt;
pub use repos::secrets::Secrets;
pub use repos::settings::Settings;
pub use repos::vehicle_states::{LastChanges, StoredVehicleState, VehicleStates};
pub use repos::vehicles::{DiscoveredVehicle, Vehicles};

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, PersistenceError>;
