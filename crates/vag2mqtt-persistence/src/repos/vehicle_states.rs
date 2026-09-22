//! The last vehicle state snapshot per VIN.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use sqlx::Row;
use vag2mqtt_domain::{VehicleState, Vin};

use crate::database::Database;
use crate::error::PersistenceError;
use crate::rows;

const TABLE: &str = "vehicle_states";

/// Last-change times per topic path, in ms since the epoch, as the MQTT publisher tracks them.
pub type LastChanges = BTreeMap<String, i64>;

/// What is stored for a vehicle.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredVehicleState {
    /// The snapshot.
    pub state: VehicleState,
    /// The publisher's last-change map at the time of storing.
    pub last_changes: LastChanges,
    /// When the row was written.
    pub updated_at: DateTime<Utc>,
}

/// Repository for the last vehicle state per VIN.
pub struct VehicleStates<'a> {
    db: &'a Database,
}

impl<'a> VehicleStates<'a> {
    pub(crate) fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// The stored snapshot, if any.
    pub async fn get(&self, vin: &Vin) -> Result<Option<StoredVehicleState>, PersistenceError> {
        let row = sqlx::query(
            "SELECT state_json, last_changes_json, updated_at FROM vehicle_states WHERE vin = ?",
        )
        .bind(vin.as_str())
        .fetch_optional(self.db.pool())
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let state_json: String = row.try_get("state_json")?;
        let last_changes_json: String = row.try_get("last_changes_json")?;
        let updated_at: String = row.try_get("updated_at")?;
        Ok(Some(StoredVehicleState {
            state: serde_json::from_str(&state_json).map_err(|e| PersistenceError::InvalidRow {
                table: TABLE,
                reason: format!("state_json: {e}"),
            })?,
            last_changes: serde_json::from_str(&last_changes_json).map_err(|e| {
                PersistenceError::InvalidRow {
                    table: TABLE,
                    reason: format!("last_changes_json: {e}"),
                }
            })?,
            updated_at: rows::parse_ts(TABLE, &updated_at)?,
        }))
    }

    /// Stores the snapshot, replacing the previous one.
    pub async fn upsert(
        &self,
        vin: &Vin,
        state: &VehicleState,
        last_changes: &LastChanges,
    ) -> Result<(), PersistenceError> {
        let invalid = |what: &str, e: serde_json::Error| PersistenceError::InvalidRow {
            table: TABLE,
            reason: format!("{what}: {e}"),
        };
        let state_json = serde_json::to_string(state).map_err(|e| invalid("state_json", e))?;
        let last_changes_json =
            serde_json::to_string(last_changes).map_err(|e| invalid("last_changes_json", e))?;
        sqlx::query(
            "INSERT INTO vehicle_states (vin, fetched_at, state_json, last_changes_json, updated_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(vin) DO UPDATE SET fetched_at = excluded.fetched_at, \
             state_json = excluded.state_json, last_changes_json = excluded.last_changes_json, \
             updated_at = excluded.updated_at",
        )
        .bind(vin.as_str())
        .bind(rows::ts(state.fetched_at))
        .bind(state_json)
        .bind(last_changes_json)
        .bind(rows::now())
        .execute(self.db.pool())
        .await?;
        Ok(())
    }
}
