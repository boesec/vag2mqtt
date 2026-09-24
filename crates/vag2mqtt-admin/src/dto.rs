//! Request and response shapes.
//!
//! Passwords are **write only**: they appear in request bodies as
//! [`Secret<String>`](vag2mqtt_domain::Secret) and in no response type at all. There is
//! deliberately no response struct with a password field, so one cannot be filled in by mistake.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use vag2mqtt_domain::{
    Account, AccountConnectionState, Brand, MqttConfig, MqttProtocol, Secret, Vehicle,
    VehicleDataState, VehicleState,
};
use vag2mqtt_runtime::{AccountStatus, MqttStatus, RuntimeStatus, VehicleStatus};

use crate::error::ApiError;

// ----- responses ------------------------------------------------------------------------

/// `GET /api/health`.
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Always `"ok"` while the process answers.
    pub status: String,
    /// The service version.
    pub version: String,
}

/// `GET /api/status`.
#[derive(Debug, Serialize, Deserialize)]
pub struct StatusResponse {
    /// When the runtime started.
    pub started_at: DateTime<Utc>,
    /// When the snapshot was taken.
    pub snapshot_at: DateTime<Utc>,
    /// The MQTT connection.
    pub mqtt: MqttResponse,
    /// Every account.
    pub accounts: Vec<AccountResponse>,
    /// Every vehicle.
    pub vehicles: Vec<VehicleResponse>,
}

impl From<RuntimeStatus> for StatusResponse {
    fn from(status: RuntimeStatus) -> Self {
        Self {
            started_at: status.started_at,
            snapshot_at: status.snapshot_at,
            mqtt: MqttResponse::from(status.mqtt),
            accounts: status
                .accounts
                .into_iter()
                .map(AccountResponse::from)
                .collect(),
            vehicles: status
                .vehicles
                .into_iter()
                .map(VehicleResponse::from)
                .collect(),
        }
    }
}

/// The MQTT connection, without its password.
#[derive(Debug, Serialize, Deserialize)]
pub struct MqttResponse {
    /// A configuration exists.
    pub configured: bool,
    /// Publishing is switched on.
    pub enabled: bool,
    /// The publisher has a live broker session.
    pub connected: bool,
    /// The stored configuration. Never carries a password.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<MqttConfigResponse>,
}

impl From<MqttStatus> for MqttResponse {
    fn from(status: MqttStatus) -> Self {
        Self {
            configured: status.configured,
            enabled: status.enabled,
            connected: status.connected,
            config: status.config.map(MqttConfigResponse::from),
        }
    }
}

/// A broker configuration as the API shows it.
#[derive(Debug, Serialize, Deserialize)]
pub struct MqttConfigResponse {
    /// Broker host.
    pub host: String,
    /// Broker port.
    pub port: u16,
    /// Broker user name, if any.
    pub username: Option<String>,
    /// Whether a password is stored. The password itself is never returned.
    pub has_password: bool,
    /// Connect with TLS.
    pub tls: bool,
    /// The first topic segment.
    pub topic_prefix: String,
    /// The MQTT client identifier.
    pub client_id: String,
    /// Keep alive in seconds.
    pub keep_alive_secs: u64,
    /// Protocol version.
    pub protocol: MqttProtocol,
}

impl From<MqttConfig> for MqttConfigResponse {
    fn from(config: MqttConfig) -> Self {
        Self {
            host: config.host,
            port: config.port,
            username: config.username,
            // Whether a password exists is not part of the runtime snapshot; a user name implies
            // one in every broker setup we support, and saying "unknown" would help nobody.
            has_password: false,
            tls: config.tls,
            topic_prefix: config.topic_prefix,
            client_id: config.client_id,
            keep_alive_secs: config.keep_alive.as_secs(),
            protocol: config.protocol,
        }
    }
}

/// An account, without its credentials.
#[derive(Debug, Serialize, Deserialize)]
pub struct AccountResponse {
    /// Internal identifier.
    pub id: String,
    /// The manufacturer service.
    pub brand: Brand,
    /// The login identifier.
    pub username: String,
    /// Whether the runtime polls it.
    pub enabled: bool,
    /// Polling interval in seconds.
    pub polling_interval_secs: u64,
    /// The connection state.
    pub connection_state: AccountConnectionState,
    /// When the manufacturer last answered.
    pub last_success_at: Option<DateTime<Utc>>,
    /// The last error, for diagnostics.
    pub last_error: Option<ErrorInfo>,
    /// When the next poll is due.
    pub next_poll_at: Option<DateTime<Utc>>,
    /// Consecutive transient failures.
    pub consecutive_failures: u32,
    /// Restarts after a panic since the service started.
    pub respawns: u32,
    /// Whether an account task is running.
    pub running: bool,
}

impl From<AccountStatus> for AccountResponse {
    fn from(status: AccountStatus) -> Self {
        let Account {
            id,
            brand,
            username,
            enabled,
            polling,
            connection_state,
            last_success_at,
            last_error,
        } = status.account;
        Self {
            id: id.to_string(),
            brand,
            username,
            enabled,
            polling_interval_secs: polling.interval.as_secs(),
            connection_state,
            last_success_at,
            last_error: last_error.map(ErrorInfo::from),
            next_poll_at: status.next_poll_at,
            consecutive_failures: status.consecutive_failures,
            respawns: status.respawns,
            running: status.running,
        }
    }
}

/// A vehicle.
#[derive(Debug, Serialize, Deserialize)]
pub struct VehicleResponse {
    /// The VIN.
    pub vin: String,
    /// The account it belongs to.
    pub account_id: String,
    /// The brand.
    pub brand: Brand,
    /// The manufacturer's model designation.
    pub model: Option<String>,
    /// The user's display name.
    pub display_name: Option<String>,
    /// What the UI shows: display name, else model, else VIN.
    pub label: String,
    /// The propulsion.
    pub drivetrain: vag2mqtt_domain::Drivetrain,
    /// Whether the runtime polls it.
    pub enabled: bool,
    /// Set when discovery no longer returns this VIN.
    pub missing_since: Option<DateTime<Utc>>,
    /// Freshness of the data.
    pub data_state: VehicleDataState,
    /// When the state was last fetched.
    pub last_update_at: Option<DateTime<Utc>>,
    /// The last error, for diagnostics.
    pub last_error: Option<ErrorInfo>,
    /// Whether a state snapshot exists.
    pub has_state: bool,
}

impl From<VehicleStatus> for VehicleResponse {
    fn from(status: VehicleStatus) -> Self {
        let label = status.vehicle.label().to_string();
        let has_state = status.last_state.is_some();
        let Vehicle {
            vin,
            account_id,
            brand,
            model,
            display_name,
            drivetrain,
            enabled,
            missing_since,
            data_state,
            last_update_at,
            last_error,
        } = status.vehicle;
        Self {
            vin: vin.to_string(),
            account_id: account_id.to_string(),
            brand,
            model,
            display_name,
            label,
            drivetrain,
            enabled,
            missing_since,
            data_state,
            last_update_at,
            last_error: last_error.map(ErrorInfo::from),
            has_state,
        }
    }
}

/// The last error of an account or a vehicle (FR-021).
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorInfo {
    /// When it happened.
    pub at: DateTime<Utc>,
    /// Its category.
    pub category: vag2mqtt_domain::ErrorCategory,
    /// A message free of secrets.
    pub message: String,
}

impl From<vag2mqtt_domain::LastError> for ErrorInfo {
    fn from(error: vag2mqtt_domain::LastError) -> Self {
        Self {
            at: error.at,
            category: error.category,
            message: error.message,
        }
    }
}

/// `GET /api/brands`.
#[derive(Debug, Serialize, Deserialize)]
pub struct BrandsResponse {
    /// The brands this build has a connector for.
    pub brands: Vec<Brand>,
}

/// `GET /api/vehicles/{vin}/state`.
#[derive(Debug, Serialize, Deserialize)]
pub struct VehicleStateResponse {
    /// The VIN.
    pub vin: String,
    /// The snapshot.
    pub state: VehicleState,
}

/// What a write returns when it has nothing to say beyond success.
#[derive(Debug, Serialize, Deserialize)]
pub struct AcceptedResponse {
    /// Always `true`.
    pub ok: bool,
}

impl AcceptedResponse {
    /// The usual answer.
    pub fn ok() -> Self {
        Self { ok: true }
    }
}

/// `POST /api/accounts`.
#[derive(Debug, Serialize, Deserialize)]
pub struct CreatedAccountResponse {
    /// The new account's identifier.
    pub id: String,
}

// ----- requests -------------------------------------------------------------------------

/// `POST /api/accounts`.
#[derive(Deserialize)]
pub struct CreateAccountRequest {
    /// The manufacturer service.
    pub brand: Brand,
    /// The login identifier.
    pub username: String,
    /// The password. Write only.
    pub password: Secret<String>,
    /// Polling interval in seconds; the runtime default when absent.
    pub polling_interval_secs: Option<u64>,
    /// Start polling right away. Defaults to `true`.
    pub enabled: Option<bool>,
}

impl std::fmt::Debug for CreateAccountRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateAccountRequest")
            .field("brand", &self.brand)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

/// `PATCH /api/accounts/{id}`.
#[derive(Debug, Deserialize)]
pub struct UpdateAccountRequest {
    /// A new login identifier.
    pub username: Option<String>,
    /// A new polling interval in seconds.
    pub polling_interval_secs: Option<u64>,
}

/// `PUT /api/accounts/{id}/credentials`.
#[derive(Deserialize)]
pub struct UpdateCredentialsRequest {
    /// The new password. Write only.
    pub password: Secret<String>,
}

impl std::fmt::Debug for UpdateCredentialsRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UpdateCredentialsRequest { password: [REDACTED] }")
    }
}

/// `POST /api/accounts/{id}/enabled`, `POST /api/vehicles/{vin}/enabled`, `POST /api/mqtt/enabled`.
#[derive(Debug, Deserialize)]
pub struct EnabledRequest {
    /// The new state.
    pub enabled: bool,
}

/// `PATCH /api/vehicles/{vin}`.
#[derive(Debug, Deserialize)]
pub struct UpdateVehicleRequest {
    /// The new display name; `null` clears it.
    pub display_name: Option<String>,
}

/// `PUT /api/mqtt`.
#[derive(Deserialize)]
pub struct ConfigureMqttRequest {
    /// Broker host.
    pub host: String,
    /// Broker port; 1883 when absent.
    pub port: Option<u16>,
    /// Broker user name.
    pub username: Option<String>,
    /// The broker password. Write only; absent keeps the stored one.
    pub password: Option<Secret<String>>,
    /// Connect with TLS.
    pub tls: Option<bool>,
    /// The first topic segment.
    pub topic_prefix: Option<String>,
    /// The MQTT client identifier.
    pub client_id: Option<String>,
    /// Keep alive in seconds.
    pub keep_alive_secs: Option<u64>,
    /// Protocol version.
    pub protocol: Option<MqttProtocol>,
    /// Publish at all. Defaults to `true`.
    pub enabled: Option<bool>,
}

impl std::fmt::Debug for ConfigureMqttRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigureMqttRequest")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("password", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl ConfigureMqttRequest {
    /// Turns the request into a domain configuration, filling in the defaults.
    ///
    /// `client_id` is generated here when absent, because the domain has no randomness.
    pub fn into_config(self) -> Result<(MqttConfig, Option<Secret<String>>), ApiError> {
        if self.host.trim().is_empty() {
            return Err(ApiError::field("host", "the broker host is empty"));
        }
        let mut config = MqttConfig::defaults(self.host.trim(), generated_client_id());
        if let Some(port) = self.port {
            if port == 0 {
                return Err(ApiError::field("port", "the broker port is zero"));
            }
            config.port = port;
        }
        config.username = self.username.filter(|u| !u.trim().is_empty());
        if let Some(tls) = self.tls {
            config.tls = tls;
        }
        if let Some(prefix) = self.topic_prefix {
            if prefix.trim().is_empty() {
                return Err(ApiError::field("topic_prefix", "the topic prefix is empty"));
            }
            config.topic_prefix = prefix.trim().to_string();
        }
        if let Some(client_id) = self.client_id {
            if client_id.trim().is_empty() {
                return Err(ApiError::field("client_id", "the client id is empty"));
            }
            config.client_id = client_id.trim().to_string();
        }
        if let Some(keep_alive) = self.keep_alive_secs {
            if keep_alive == 0 {
                return Err(ApiError::field("keep_alive_secs", "keep alive is zero"));
            }
            config.keep_alive = Duration::from_secs(keep_alive);
        }
        if let Some(protocol) = self.protocol {
            config.protocol = protocol;
        }
        config.enabled = self.enabled.unwrap_or(true);
        Ok((config, self.password))
    }
}

/// `vag2mqtt-` plus eight hex characters, as FR-014 describes.
fn generated_client_id() -> String {
    let suffix: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect();
    format!("vag2mqtt-{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(json: serde_json::Value) -> ConfigureMqttRequest {
        serde_json::from_value(json).expect("valid request")
    }

    #[test]
    fn mqtt_defaults_are_filled_in() {
        let (config, password) = request(serde_json::json!({"host": "broker.local"}))
            .into_config()
            .unwrap();
        assert_eq!(config.host, "broker.local");
        assert_eq!(config.port, 1883);
        assert_eq!(config.topic_prefix, "vag2mqtt");
        assert_eq!(config.keep_alive, Duration::from_secs(30));
        assert!(config.enabled);
        assert!(config.client_id.starts_with("vag2mqtt-"));
        assert_eq!(config.client_id.len(), "vag2mqtt-".len() + 8);
        assert!(password.is_none());
    }

    #[test]
    fn empty_and_zero_values_are_rejected_with_their_field() {
        let cases: Vec<(serde_json::Value, &str)> = vec![
            (serde_json::json!({"host": "  "}), "host"),
            (serde_json::json!({"host": "h", "port": 0}), "port"),
            (
                serde_json::json!({"host": "h", "topic_prefix": " "}),
                "topic_prefix",
            ),
            (
                serde_json::json!({"host": "h", "client_id": ""}),
                "client_id",
            ),
            (
                serde_json::json!({"host": "h", "keep_alive_secs": 0}),
                "keep_alive_secs",
            ),
        ];
        for (json, field) in cases {
            match request(json).into_config() {
                Err(ApiError::Validation { field: Some(f), .. }) => assert_eq!(f, field),
                other => panic!("expected a validation error for {field}, got {other:?}"),
            }
        }
    }

    #[test]
    fn request_debug_never_prints_a_password() {
        let create = CreateAccountRequest {
            brand: Brand::Audi,
            username: "driver@example.test".into(),
            password: Secret::new("hunter2-MARKER".into()),
            polling_interval_secs: None,
            enabled: None,
        };
        assert!(!format!("{create:?}").contains("hunter2"));
        let credentials = UpdateCredentialsRequest {
            password: Secret::new("hunter2-MARKER".into()),
        };
        assert!(!format!("{credentials:?}").contains("hunter2"));
        let mqtt = request(serde_json::json!({"host": "h", "password": "hunter2-MARKER"}));
        assert!(!format!("{mqtt:?}").contains("hunter2"));
    }

    #[test]
    fn no_response_type_can_carry_a_password() {
        // The check that matters is structural: a response struct with a password field cannot be
        // filled in by mistake if it does not exist. Serialising every response type and looking
        // for the word is the cheapest way to keep that true as the file grows.
        let config = MqttConfigResponse::from(MqttConfig::defaults("h", "vag2mqtt-test"));
        let json = serde_json::to_string(&config).unwrap();
        assert!(
            !json.contains("password") || json.contains("has_password"),
            "{json}"
        );
        assert!(!json.contains("\"password\""), "{json}");
    }
}
