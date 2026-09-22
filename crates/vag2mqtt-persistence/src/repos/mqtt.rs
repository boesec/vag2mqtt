//! The MQTT connection configuration.

use std::time::Duration;

use sqlx::Row;
use vag2mqtt_domain::{MqttConfig, MqttProtocol};

use crate::database::Database;
use crate::error::PersistenceError;
use crate::repos::accounts::not_found_if_zero;
use crate::repos::secrets::{Owner, delete_all_for};
use crate::rows;

const TABLE: &str = "mqtt_connections";
const DEFAULT_ID: &str = "default";

/// Repository for the (single, in this release) MQTT connection.
pub struct Mqtt<'a> {
    db: &'a Database,
}

impl<'a> Mqtt<'a> {
    pub(crate) fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// The stored configuration, without the password (see
    /// [`Secrets::mqtt_password`](crate::Secrets::mqtt_password)).
    pub async fn get(&self) -> Result<Option<MqttConfig>, PersistenceError> {
        let row = sqlx::query("SELECT * FROM mqtt_connections WHERE id = ?")
            .bind(DEFAULT_ID)
            .fetch_optional(self.db.pool())
            .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let port: i64 = row.try_get("port")?;
        let keep_alive_secs: i64 = row.try_get("keep_alive_secs")?;
        let protocol: String = row.try_get("protocol")?;
        Ok(Some(MqttConfig {
            enabled: row.try_get("enabled")?,
            host: row.try_get("host")?,
            port: u16::try_from(port).map_err(|_| PersistenceError::InvalidRow {
                table: TABLE,
                reason: "port".to_string(),
            })?,
            username: row.try_get("username")?,
            tls: row.try_get("tls")?,
            topic_prefix: row.try_get("topic_prefix")?,
            client_id: row.try_get("client_id")?,
            keep_alive: Duration::from_secs(u64::try_from(keep_alive_secs).unwrap_or(0)),
            protocol: rows::parse_enum::<MqttProtocol>(TABLE, "protocol", &protocol)?,
        }))
    }

    /// Stores the configuration, replacing the previous one.
    pub async fn upsert(&self, config: &MqttConfig) -> Result<(), PersistenceError> {
        sqlx::query(
            "INSERT INTO mqtt_connections (id, enabled, host, port, username, tls, topic_prefix, \
             client_id, keep_alive_secs, protocol, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled, host = excluded.host, \
             port = excluded.port, username = excluded.username, tls = excluded.tls, \
             topic_prefix = excluded.topic_prefix, client_id = excluded.client_id, \
             keep_alive_secs = excluded.keep_alive_secs, protocol = excluded.protocol, \
             updated_at = excluded.updated_at",
        )
        .bind(DEFAULT_ID)
        .bind(config.enabled)
        .bind(&config.host)
        .bind(i64::from(config.port))
        .bind(&config.username)
        .bind(config.tls)
        .bind(&config.topic_prefix)
        .bind(&config.client_id)
        .bind(i64::try_from(config.keep_alive.as_secs()).unwrap_or(i64::MAX))
        .bind(rows::enum_str(&config.protocol))
        .bind(rows::now())
        .execute(self.db.pool())
        .await?;
        Ok(())
    }

    /// Enables or disables the connection without touching the rest.
    pub async fn set_enabled(&self, enabled: bool) -> Result<(), PersistenceError> {
        let result =
            sqlx::query("UPDATE mqtt_connections SET enabled = ?, updated_at = ? WHERE id = ?")
                .bind(enabled)
                .bind(rows::now())
                .bind(DEFAULT_ID)
                .execute(self.db.pool())
                .await?;
        not_found_if_zero(result.rows_affected(), "mqtt connection")
    }

    /// Removes the configuration and its password.
    pub async fn delete(&self) -> Result<(), PersistenceError> {
        let mut tx = self.db.pool().begin().await?;
        delete_all_for(&mut tx, &Owner::Mqtt).await?;
        sqlx::query("DELETE FROM mqtt_connections WHERE id = ?")
            .bind(DEFAULT_ID)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
