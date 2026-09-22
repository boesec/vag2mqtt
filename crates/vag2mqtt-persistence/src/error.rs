//! Errors of the persistence layer.

use std::path::PathBuf;

/// Everything that can go wrong in this crate.
///
/// Messages never carry secrets, full VINs or key material.
#[derive(Debug, thiserror::Error)]
pub enum PersistenceError {
    /// A migration failed to apply.
    #[error("database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    /// A query failed.
    #[error("database query failed: {0}")]
    Query(#[from] sqlx::Error),
    /// The master key does not decrypt the key check stored in this database.
    #[error(
        "the master key does not match this database; check VAG2MQTT_MASTER_KEY or the key file"
    )]
    MasterKeyMismatch,
    /// The provided master key is malformed.
    #[error("invalid master key: {reason}")]
    MasterKeyInvalid {
        /// Why it was rejected.
        reason: &'static str,
    },
    /// The key file could not be read or written.
    #[error("master key file {path}: {source}")]
    KeyFile {
        /// The key file.
        path: PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A stored secret failed authentication.
    #[error("could not decrypt secret of {owner}")]
    Decrypt {
        /// A description of the owner row, for example `account 3f25...` or `mqtt`.
        owner: String,
    },
    /// A stored value no longer parses as its domain type.
    #[error("invalid row in table {table}: {reason}")]
    InvalidRow {
        /// The table.
        table: &'static str,
        /// What did not parse.
        reason: String,
    },
    /// A VIN was discovered under a second account. A VIN belongs to exactly one account.
    #[error("vehicle {vin_short} already belongs to another account")]
    VinOwnedByOtherAccount {
        /// The VIN, shortened to its last four characters.
        vin_short: String,
    },
    /// The referenced row does not exist.
    #[error("{entity} not found")]
    NotFound {
        /// What was looked up, for example `account` or `vehicle`.
        entity: &'static str,
    },
}
