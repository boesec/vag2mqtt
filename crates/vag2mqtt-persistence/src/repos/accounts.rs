//! Accounts.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;
use vag2mqtt_domain::{
    Account, AccountConnectionState, AccountId, Brand, LastError, PollingConfig, Secret,
};

use crate::database::Database;
use crate::error::PersistenceError;
use crate::repos::secrets::{Kind, Owner, delete_all_for};
use crate::rows;

const TABLE: &str = "accounts";

/// Input for creating an account.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewAccount {
    /// The manufacturer service.
    pub brand: Brand,
    /// The login identifier.
    pub username: String,
    /// Start polling right away.
    pub enabled: bool,
    /// The polling interval.
    pub polling: PollingConfig,
}

/// Fields of an account the user may change after creation. `None` keeps the current value.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountConfigPatch {
    /// A new login identifier.
    pub username: Option<String>,
    /// A new polling configuration.
    pub polling: Option<PollingConfig>,
}

/// Account repository.
pub struct Accounts<'a> {
    db: &'a Database,
}

impl<'a> Accounts<'a> {
    pub(crate) fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// Every account, ordered by creation time.
    pub async fn list(&self) -> Result<Vec<Account>, PersistenceError> {
        let rows = sqlx::query("SELECT * FROM accounts ORDER BY created_at, id")
            .fetch_all(self.db.pool())
            .await?;
        rows.iter().map(account_from_row).collect()
    }

    /// One account by id.
    pub async fn get(&self, id: &AccountId) -> Result<Option<Account>, PersistenceError> {
        let row = sqlx::query("SELECT * FROM accounts WHERE id = ?")
            .bind(id.as_str())
            .fetch_optional(self.db.pool())
            .await?;
        row.as_ref().map(account_from_row).transpose()
    }

    /// Creates an account and stores its password in the same transaction. Returns the new
    /// record with a generated id and `connection_state = Pending`.
    pub async fn insert(
        &self,
        new: NewAccount,
        password: &Secret<String>,
    ) -> Result<Account, PersistenceError> {
        let id = AccountId::new(uuid::Uuid::new_v4().to_string()).map_err(|e| {
            PersistenceError::InvalidRow {
                table: TABLE,
                reason: format!("generated id: {e}"),
            }
        })?;
        let now = rows::now();
        let interval_secs = i64::try_from(new.polling.interval.as_secs()).unwrap_or(i64::MAX);

        let mut tx = self.db.pool().begin().await?;
        sqlx::query(
            "INSERT INTO accounts (id, brand, username, enabled, polling_interval_secs, \
             connection_state, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id.as_str())
        .bind(rows::enum_str(&new.brand))
        .bind(&new.username)
        .bind(new.enabled)
        .bind(interval_secs)
        .bind(rows::enum_str(&AccountConnectionState::Pending))
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        self.db
            .secrets()
            .store(
                &mut tx,
                &Owner::Account(id.clone()),
                Kind::Password,
                password.expose_secret().as_bytes(),
            )
            .await?;
        tx.commit().await?;

        Ok(Account {
            id,
            brand: new.brand,
            username: new.username,
            enabled: new.enabled,
            polling: new.polling,
            connection_state: AccountConnectionState::Pending,
            last_success_at: None,
            last_error: None,
        })
    }

    /// Applies a configuration patch.
    pub async fn update_config(
        &self,
        id: &AccountId,
        patch: AccountConfigPatch,
    ) -> Result<(), PersistenceError> {
        let interval_secs = patch
            .polling
            .map(|p| i64::try_from(p.interval.as_secs()).unwrap_or(i64::MAX));
        let result = sqlx::query(
            "UPDATE accounts SET \
             username = COALESCE(?, username), \
             polling_interval_secs = COALESCE(?, polling_interval_secs), \
             updated_at = ? WHERE id = ?",
        )
        .bind(patch.username)
        .bind(interval_secs)
        .bind(rows::now())
        .bind(id.as_str())
        .execute(self.db.pool())
        .await?;
        not_found_if_zero(result.rows_affected(), "account")
    }

    /// Enables or disables an account.
    pub async fn set_enabled(&self, id: &AccountId, enabled: bool) -> Result<(), PersistenceError> {
        let result = sqlx::query("UPDATE accounts SET enabled = ?, updated_at = ? WHERE id = ?")
            .bind(enabled)
            .bind(rows::now())
            .bind(id.as_str())
            .execute(self.db.pool())
            .await?;
        not_found_if_zero(result.rows_affected(), "account")
    }

    /// Mirrors the runtime's view of the account (FR-021).
    pub async fn update_runtime_status(
        &self,
        id: &AccountId,
        state: AccountConnectionState,
        last_success_at: Option<DateTime<Utc>>,
        last_error: Option<&LastError>,
    ) -> Result<(), PersistenceError> {
        let (error_at, error_category, error_message) = rows::last_error_columns(last_error);
        let result = sqlx::query(
            "UPDATE accounts SET connection_state = ?, last_success_at = ?, last_error_at = ?, \
             last_error_category = ?, last_error_message = ?, updated_at = ? WHERE id = ?",
        )
        .bind(rows::enum_str(&state))
        .bind(last_success_at.map(rows::ts))
        .bind(error_at)
        .bind(error_category)
        .bind(error_message)
        .bind(rows::now())
        .bind(id.as_str())
        .execute(self.db.pool())
        .await?;
        not_found_if_zero(result.rows_affected(), "account")
    }

    /// Deletes the account, its vehicles, their states and every secret of the account.
    pub async fn delete(&self, id: &AccountId) -> Result<(), PersistenceError> {
        let mut tx = self.db.pool().begin().await?;
        delete_all_for(&mut tx, &Owner::Account(id.clone())).await?;
        let result = sqlx::query("DELETE FROM accounts WHERE id = ?")
            .bind(id.as_str())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        not_found_if_zero(result.rows_affected(), "account")
    }
}

pub(crate) fn not_found_if_zero(
    rows_affected: u64,
    entity: &'static str,
) -> Result<(), PersistenceError> {
    if rows_affected == 0 {
        Err(PersistenceError::NotFound { entity })
    } else {
        Ok(())
    }
}

fn account_from_row(row: &SqliteRow) -> Result<Account, PersistenceError> {
    let id: String = row.try_get("id")?;
    let brand: String = row.try_get("brand")?;
    let interval_secs: i64 = row.try_get("polling_interval_secs")?;
    let connection_state: String = row.try_get("connection_state")?;
    let last_success_at: Option<String> = row.try_get("last_success_at")?;
    let last_error_at: Option<String> = row.try_get("last_error_at")?;
    let last_error_category: Option<String> = row.try_get("last_error_category")?;
    let last_error_message: Option<String> = row.try_get("last_error_message")?;

    Ok(Account {
        id: AccountId::new(id).map_err(|e| PersistenceError::InvalidRow {
            table: TABLE,
            reason: format!("id: {e}"),
        })?,
        brand: rows::parse_enum(TABLE, "brand", &brand)?,
        username: row.try_get("username")?,
        enabled: row.try_get("enabled")?,
        polling: PollingConfig {
            interval: Duration::from_secs(u64::try_from(interval_secs).unwrap_or(0)),
        },
        connection_state: rows::parse_enum(TABLE, "connection_state", &connection_state)?,
        last_success_at: rows::parse_opt_ts(TABLE, last_success_at.as_deref())?,
        last_error: rows::parse_last_error(
            TABLE,
            last_error_at.as_deref(),
            last_error_category.as_deref(),
            last_error_message.as_deref(),
        )?,
    })
}
