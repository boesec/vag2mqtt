//! The database handle: pool, cipher and startup checks.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};

use crate::crypto::{Cipher, EncryptedBlob};
use crate::error::PersistenceError;
use crate::master_key::{MasterKey, MasterKeySource};
use crate::repos::accounts::Accounts;
use crate::repos::mqtt::Mqtt;
use crate::repos::secrets::Secrets;
use crate::repos::settings::Settings;
use crate::repos::vehicle_states::VehicleStates;
use crate::repos::vehicles::Vehicles;

/// Name of the database file inside the data directory.
pub const DB_FILE_NAME: &str = "vag2mqtt.db";

const KEY_CHECK_SETTING: &str = "key_check";
const KEY_CHECK_AAD: &[u8] = b"settings\0key_check";
const KEY_CHECK_PLAINTEXT: &[u8] = b"vag2mqtt-key-check";

/// A handle to the SQLite database and the secret cipher. Cheap to clone.
#[derive(Clone)]
pub struct Database {
    pool: SqlitePool,
    cipher: Arc<Cipher>,
    key_file: Option<PathBuf>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database")
            .field("key_file", &self.key_file)
            .finish_non_exhaustive()
    }
}

impl Database {
    /// Opens (or creates) `<data_dir>/vag2mqtt.db`, resolves the master key, applies pending
    /// migrations and verifies that the key matches the database.
    pub async fn open(
        data_dir: &Path,
        key_source: MasterKeySource,
    ) -> Result<Self, PersistenceError> {
        std::fs::create_dir_all(data_dir).map_err(|source| PersistenceError::KeyFile {
            path: data_dir.to_path_buf(),
            source,
        })?;
        let key = MasterKey::resolve(data_dir, key_source)?;
        let cipher = Arc::new(Cipher::new(&key));

        let db_path = data_dir.join(DB_FILE_NAME);
        let options = SqliteConnectOptions::from_str("sqlite://")?
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(options)
            .await?;

        sqlx::migrate!("./migrations").run(&pool).await?;

        let db = Self {
            pool,
            cipher,
            key_file: key.file,
        };
        db.verify_key_check().await?;
        tracing::info!(
            target: "vag2mqtt::persistence",
            path = %db_path.display(),
            "database ready"
        );
        Ok(db)
    }

    /// Where the key file lives, if the key came from a file.
    pub fn key_file_path(&self) -> Option<&Path> {
        self.key_file.as_deref()
    }

    /// Closes the pool. Pending writes are flushed.
    pub async fn close(&self) {
        self.pool.close().await;
    }

    /// Account repository.
    pub fn accounts(&self) -> Accounts<'_> {
        Accounts::new(self)
    }

    /// Vehicle repository.
    pub fn vehicles(&self) -> Vehicles<'_> {
        Vehicles::new(self)
    }

    /// Last vehicle state per VIN.
    pub fn vehicle_states(&self) -> VehicleStates<'_> {
        VehicleStates::new(self)
    }

    /// MQTT connection configuration.
    pub fn mqtt(&self) -> Mqtt<'_> {
        Mqtt::new(self)
    }

    /// Encrypted secrets.
    pub fn secrets(&self) -> Secrets<'_> {
        Secrets::new(self)
    }

    /// Key/value settings.
    pub fn settings(&self) -> Settings<'_> {
        Settings::new(self)
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub(crate) fn cipher(&self) -> &Cipher {
        &self.cipher
    }

    /// Stores the key check on first start and verifies it on every later start.
    async fn verify_key_check(&self) -> Result<(), PersistenceError> {
        let stored: Option<StoredKeyCheck> = self.settings().get(KEY_CHECK_SETTING).await?;
        match stored {
            None => {
                let blob = self.cipher.encrypt(KEY_CHECK_AAD, KEY_CHECK_PLAINTEXT);
                self.settings()
                    .set(KEY_CHECK_SETTING, &StoredKeyCheck::from(&blob))
                    .await
            }
            Some(check) => {
                let blob = check.into_blob()?;
                let plain = self
                    .cipher
                    .decrypt(KEY_CHECK_AAD, &blob)
                    .map_err(|_| PersistenceError::MasterKeyMismatch)?;
                if plain.as_slice() == KEY_CHECK_PLAINTEXT {
                    Ok(())
                } else {
                    Err(PersistenceError::MasterKeyMismatch)
                }
            }
        }
    }
}

/// The key check blob in its settings JSON form.
#[derive(serde::Serialize, serde::Deserialize)]
struct StoredKeyCheck {
    key_version: u32,
    nonce: String,
    ciphertext: String,
}

impl From<&EncryptedBlob> for StoredKeyCheck {
    fn from(blob: &EncryptedBlob) -> Self {
        Self {
            key_version: blob.key_version,
            nonce: hex::encode(blob.nonce),
            ciphertext: hex::encode(&blob.ciphertext),
        }
    }
}

impl StoredKeyCheck {
    fn into_blob(self) -> Result<EncryptedBlob, PersistenceError> {
        let invalid = |reason: &str| PersistenceError::InvalidRow {
            table: "settings",
            reason: format!("key_check: {reason}"),
        };
        let nonce_bytes = hex::decode(&self.nonce).map_err(|_| invalid("nonce"))?;
        let nonce: [u8; crate::crypto::NONCE_LEN] = nonce_bytes
            .as_slice()
            .try_into()
            .map_err(|_| invalid("nonce length"))?;
        let ciphertext = hex::decode(&self.ciphertext).map_err(|_| invalid("ciphertext"))?;
        Ok(EncryptedBlob {
            key_version: self.key_version,
            nonce,
            ciphertext,
        })
    }
}
