//! Key/value settings stored as JSON.

use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::Row;

use crate::database::Database;
use crate::error::PersistenceError;
use crate::rows;

/// Global settings (log filter, stale multiplier, ...), each a JSON value under a key.
pub struct Settings<'a> {
    db: &'a Database,
}

impl<'a> Settings<'a> {
    pub(crate) fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Reads and parses a setting. `None` when the key does not exist.
    pub async fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, PersistenceError> {
        let row = sqlx::query("SELECT value_json FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(self.db.pool())
            .await?;
        row.map(|row| {
            let json: String = row.try_get("value_json")?;
            serde_json::from_str(&json).map_err(|e| PersistenceError::InvalidRow {
                table: "settings",
                reason: format!("{key}: {e}"),
            })
        })
        .transpose()
    }

    /// Writes a setting, replacing any previous value.
    pub async fn set<T: Serialize + ?Sized>(
        &self,
        key: &str,
        value: &T,
    ) -> Result<(), PersistenceError> {
        let json = serde_json::to_string(value).map_err(|e| PersistenceError::InvalidRow {
            table: "settings",
            reason: format!("{key}: {e}"),
        })?;
        sqlx::query(
            "INSERT INTO settings (key, value_json, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, \
             updated_at = excluded.updated_at",
        )
        .bind(key)
        .bind(json)
        .bind(rows::now())
        .execute(self.db.pool())
        .await?;
        Ok(())
    }

    /// Removes a setting. Removing a missing key is not an error.
    pub async fn delete(&self, key: &str) -> Result<(), PersistenceError> {
        sqlx::query("DELETE FROM settings WHERE key = ?")
            .bind(key)
            .execute(self.db.pool())
            .await?;
        Ok(())
    }
}
