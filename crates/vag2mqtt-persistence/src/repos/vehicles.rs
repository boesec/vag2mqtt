//! Vehicles.

use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use vag2mqtt_domain::{AccountId, Drivetrain, LastError, Vehicle, VehicleDataState, Vin};

use crate::database::Database;
use crate::error::PersistenceError;
use crate::repos::accounts::not_found_if_zero;
use crate::rows;

const TABLE: &str = "vehicles";

/// What discovery learned about a vehicle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredVehicle {
    /// The VIN.
    pub vin: Vin,
    /// The manufacturer's model designation, if delivered.
    pub model: Option<String>,
    /// The propulsion.
    pub drivetrain: Drivetrain,
}

/// Vehicle repository.
pub struct Vehicles<'a> {
    db: &'a Database,
}

impl<'a> Vehicles<'a> {
    pub(crate) fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Every vehicle, ordered by account and VIN.
    pub async fn list(&self) -> Result<Vec<Vehicle>, PersistenceError> {
        let rows = sqlx::query("SELECT * FROM vehicles ORDER BY account_id, vin")
            .fetch_all(self.db.pool())
            .await?;
        rows.iter().map(vehicle_from_row).collect()
    }

    /// The vehicles of one account, ordered by VIN.
    pub async fn list_for_account(
        &self,
        account_id: &AccountId,
    ) -> Result<Vec<Vehicle>, PersistenceError> {
        let rows = sqlx::query("SELECT * FROM vehicles WHERE account_id = ? ORDER BY vin")
            .bind(account_id.as_str())
            .fetch_all(self.db.pool())
            .await?;
        rows.iter().map(vehicle_from_row).collect()
    }

    /// One vehicle by VIN.
    pub async fn get(&self, vin: &Vin) -> Result<Option<Vehicle>, PersistenceError> {
        let row = sqlx::query("SELECT * FROM vehicles WHERE vin = ?")
            .bind(vin.as_str())
            .fetch_optional(self.db.pool())
            .await?;
        row.as_ref().map(vehicle_from_row).transpose()
    }

    /// Records a discovered vehicle. A new VIN is inserted enabled; a known VIN keeps its
    /// `display_name`, `enabled` and runtime status, gets `model` and `drivetrain` refreshed and
    /// `missing_since` cleared. A VIN that belongs to another account is rejected.
    pub async fn upsert_discovered(
        &self,
        account_id: &AccountId,
        discovered: &DiscoveredVehicle,
    ) -> Result<Vehicle, PersistenceError> {
        let mut tx = self.db.pool().begin().await?;

        let owner: Option<String> = sqlx::query("SELECT account_id FROM vehicles WHERE vin = ?")
            .bind(discovered.vin.as_str())
            .fetch_optional(&mut *tx)
            .await?
            .map(|row| row.try_get("account_id"))
            .transpose()?;
        if let Some(owner) = &owner
            && owner != account_id.as_str()
        {
            tracing::warn!(
                target: "vag2mqtt::persistence",
                account_id = %account_id,
                vin = %discovered.vin.short(),
                "vehicle discovered under a second account; it stays with its first account"
            );
            return Err(PersistenceError::VinOwnedByOtherAccount {
                vin_short: discovered.vin.short(),
            });
        }

        let brand: String = sqlx::query("SELECT brand FROM accounts WHERE id = ?")
            .bind(account_id.as_str())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(PersistenceError::NotFound { entity: "account" })?
            .try_get("brand")?;

        let now = rows::now();
        if owner.is_some() {
            sqlx::query(
                "UPDATE vehicles SET model = ?, drivetrain = ?, missing_since = NULL, updated_at = ? \
                 WHERE vin = ?",
            )
            .bind(&discovered.model)
            .bind(rows::enum_str(&discovered.drivetrain))
            .bind(&now)
            .bind(discovered.vin.as_str())
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "INSERT INTO vehicles (vin, account_id, brand, model, display_name, drivetrain, \
                 enabled, missing_since, data_state, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, NULL, ?, 1, NULL, ?, ?, ?)",
            )
            .bind(discovered.vin.as_str())
            .bind(account_id.as_str())
            .bind(&brand)
            .bind(&discovered.model)
            .bind(rows::enum_str(&discovered.drivetrain))
            .bind(rows::enum_str(&VehicleDataState::Pending))
            .bind(&now)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }

        let row = sqlx::query("SELECT * FROM vehicles WHERE vin = ?")
            .bind(discovered.vin.as_str())
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        vehicle_from_row(&row)
    }

    /// Marks a vehicle as no longer returned by discovery (FR-005). Idempotent: an already
    /// missing vehicle keeps its original `missing_since`.
    pub async fn mark_missing(
        &self,
        vin: &Vin,
        since: DateTime<Utc>,
    ) -> Result<(), PersistenceError> {
        let result = sqlx::query(
            "UPDATE vehicles SET missing_since = COALESCE(missing_since, ?), updated_at = ? \
             WHERE vin = ?",
        )
        .bind(rows::ts(since))
        .bind(rows::now())
        .bind(vin.as_str())
        .execute(self.db.pool())
        .await?;
        not_found_if_zero(result.rows_affected(), "vehicle")
    }

    /// Enables or disables a vehicle.
    pub async fn set_enabled(&self, vin: &Vin, enabled: bool) -> Result<(), PersistenceError> {
        let result = sqlx::query("UPDATE vehicles SET enabled = ?, updated_at = ? WHERE vin = ?")
            .bind(enabled)
            .bind(rows::now())
            .bind(vin.as_str())
            .execute(self.db.pool())
            .await?;
        not_found_if_zero(result.rows_affected(), "vehicle")
    }

    /// Sets or clears the user defined display name.
    pub async fn set_display_name(
        &self,
        vin: &Vin,
        display_name: Option<&str>,
    ) -> Result<(), PersistenceError> {
        let result =
            sqlx::query("UPDATE vehicles SET display_name = ?, updated_at = ? WHERE vin = ?")
                .bind(display_name)
                .bind(rows::now())
                .bind(vin.as_str())
                .execute(self.db.pool())
                .await?;
        not_found_if_zero(result.rows_affected(), "vehicle")
    }

    /// Mirrors the runtime's view of the vehicle (FR-021).
    pub async fn update_runtime_status(
        &self,
        vin: &Vin,
        state: VehicleDataState,
        last_update_at: Option<DateTime<Utc>>,
        last_error: Option<&LastError>,
    ) -> Result<(), PersistenceError> {
        let (error_at, error_category, error_message) = rows::last_error_columns(last_error);
        let result = sqlx::query(
            "UPDATE vehicles SET data_state = ?, last_update_at = ?, last_error_at = ?, \
             last_error_category = ?, last_error_message = ?, updated_at = ? WHERE vin = ?",
        )
        .bind(rows::enum_str(&state))
        .bind(last_update_at.map(rows::ts))
        .bind(error_at)
        .bind(error_category)
        .bind(error_message)
        .bind(rows::now())
        .bind(vin.as_str())
        .execute(self.db.pool())
        .await?;
        not_found_if_zero(result.rows_affected(), "vehicle")
    }

    /// Deletes a vehicle and its stored state.
    pub async fn delete(&self, vin: &Vin) -> Result<(), PersistenceError> {
        let result = sqlx::query("DELETE FROM vehicles WHERE vin = ?")
            .bind(vin.as_str())
            .execute(self.db.pool())
            .await?;
        not_found_if_zero(result.rows_affected(), "vehicle")
    }
}

fn vehicle_from_row(row: &SqliteRow) -> Result<Vehicle, PersistenceError> {
    let vin: String = row.try_get("vin")?;
    let account_id: String = row.try_get("account_id")?;
    let brand: String = row.try_get("brand")?;
    let drivetrain: String = row.try_get("drivetrain")?;
    let missing_since: Option<String> = row.try_get("missing_since")?;
    let data_state: String = row.try_get("data_state")?;
    let last_update_at: Option<String> = row.try_get("last_update_at")?;
    let last_error_at: Option<String> = row.try_get("last_error_at")?;
    let last_error_category: Option<String> = row.try_get("last_error_category")?;
    let last_error_message: Option<String> = row.try_get("last_error_message")?;

    Ok(Vehicle {
        vin: Vin::new(vin).map_err(|e| PersistenceError::InvalidRow {
            table: TABLE,
            reason: format!("vin: {e}"),
        })?,
        account_id: AccountId::new(account_id).map_err(|e| PersistenceError::InvalidRow {
            table: TABLE,
            reason: format!("account_id: {e}"),
        })?,
        brand: rows::parse_enum(TABLE, "brand", &brand)?,
        model: row.try_get("model")?,
        display_name: row.try_get("display_name")?,
        drivetrain: rows::parse_enum(TABLE, "drivetrain", &drivetrain)?,
        enabled: row.try_get("enabled")?,
        missing_since: rows::parse_opt_ts(TABLE, missing_since.as_deref())?,
        data_state: rows::parse_enum(TABLE, "data_state", &data_state)?,
        last_update_at: rows::parse_opt_ts(TABLE, last_update_at.as_deref())?,
        last_error: rows::parse_last_error(
            TABLE,
            last_error_at.as_deref(),
            last_error_category.as_deref(),
            last_error_message.as_deref(),
        )?,
    })
}
