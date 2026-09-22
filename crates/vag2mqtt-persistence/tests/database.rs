//! Integration tests against temporary SQLite files.

// Test helpers may panic on malformed fixtures; helper functions here are test code too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use sqlx::Row;
use vag2mqtt_domain::{
    AccountConnectionState, Brand, Drivetrain, ErrorCategory, LastError, MqttConfig, MqttProtocol,
    PollingConfig, Reading, Secret, VehicleDataState, VehicleState, Vin,
};
use vag2mqtt_persistence::{
    AccountConfigPatch, Database, DiscoveredVehicle, MasterKeySource, NewAccount, PersistenceError,
};

const PASSWORD_MARKER: &str = "PLAINTEXT-PASSWORD-MARKER-7f3a";
const SESSION_MARKER: &str = "PLAINTEXT-SESSION-MARKER-9c1e";

fn key(byte: u8) -> MasterKeySource {
    MasterKeySource::Raw(Secret::new([byte; 32]))
}

fn new_account(username: &str) -> NewAccount {
    NewAccount {
        brand: Brand::Audi,
        username: username.to_string(),
        enabled: true,
        polling: PollingConfig {
            interval: Duration::from_secs(300),
        },
    }
}

fn vin(suffix: &str) -> Vin {
    Vin::new(format!("WAUZZZ000000{suffix}")).unwrap()
}

fn discovered(suffix: &str) -> DiscoveredVehicle {
    DiscoveredVehicle {
        vin: vin(suffix),
        model: Some("A6 e-tron".to_string()),
        drivetrain: Drivetrain::Electric,
    }
}

fn sample_state() -> VehicleState {
    let fetched_at = Utc
        .with_ymd_and_hms(2026, 9, 22, 12, 0, 0)
        .single()
        .unwrap();
    let mut state = VehicleState::unsupported(fetched_at);
    state.odometer = Reading::present(vag2mqtt_domain::units::Kilometres::new(4242), None);
    state
}

#[tokio::test]
async fn open_creates_files_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path(), MasterKeySource::KeyFile)
        .await
        .unwrap();
    assert!(dir.path().join("vag2mqtt.db").exists());
    assert!(dir.path().join("master.key").exists());
    assert_eq!(
        db.key_file_path(),
        Some(dir.path().join("master.key").as_path())
    );
    db.close().await;

    let db = Database::open(dir.path(), MasterKeySource::KeyFile)
        .await
        .unwrap();
    let pool = raw_pool(dir.path()).await;
    let migrations: i64 = sqlx::query("SELECT COUNT(*) AS n FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .unwrap()
        .try_get("n")
        .unwrap();
    assert_eq!(migrations, 1);
    db.close().await;
}

#[tokio::test]
async fn restart_restores_everything_including_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let account_id;
    {
        let db = Database::open(dir.path(), key(1)).await.unwrap();
        let account = db
            .accounts()
            .insert(
                new_account("driver@example.test"),
                &Secret::new(PASSWORD_MARKER.into()),
            )
            .await
            .unwrap();
        account_id = account.id.clone();
        db.secrets()
            .set_account_session(&account.id, SESSION_MARKER.as_bytes())
            .await
            .unwrap();
        let vehicle = db
            .vehicles()
            .upsert_discovered(&account.id, &discovered("0TEST"))
            .await
            .unwrap();
        db.vehicles()
            .set_display_name(&vehicle.vin, Some("Family car"))
            .await
            .unwrap();
        let mut changes = BTreeMap::new();
        changes.insert("odometer".to_string(), 1_700_000_000_000_i64);
        db.vehicle_states()
            .upsert(&vehicle.vin, &sample_state(), &changes)
            .await
            .unwrap();
        db.mqtt()
            .upsert(&MqttConfig::defaults("broker.local", "vag2mqtt-test"))
            .await
            .unwrap();
        db.secrets()
            .set_mqtt_password(&Secret::new("mqtt-secret".into()))
            .await
            .unwrap();
        db.settings().set("stale_multiplier", &3u32).await.unwrap();
        db.close().await;
    }

    let db = Database::open(dir.path(), key(1)).await.unwrap();
    let accounts = db.accounts().list().await.unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].id, account_id);
    assert_eq!(accounts[0].username, "driver@example.test");
    assert_eq!(
        accounts[0].connection_state,
        AccountConnectionState::Pending
    );
    assert_eq!(
        db.secrets()
            .account_password(&account_id)
            .await
            .unwrap()
            .unwrap()
            .expose_secret(),
        PASSWORD_MARKER
    );
    assert_eq!(
        db.secrets()
            .account_session(&account_id)
            .await
            .unwrap()
            .unwrap()
            .expose_secret(),
        SESSION_MARKER.as_bytes()
    );
    let vehicles = db.vehicles().list_for_account(&account_id).await.unwrap();
    assert_eq!(vehicles.len(), 1);
    assert_eq!(vehicles[0].display_name.as_deref(), Some("Family car"));
    assert_eq!(vehicles[0].brand, Brand::Audi);
    let stored = db
        .vehicle_states()
        .get(&vehicles[0].vin)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, sample_state());
    assert_eq!(stored.last_changes["odometer"], 1_700_000_000_000_i64);
    let mqtt = db.mqtt().get().await.unwrap().unwrap();
    assert_eq!(mqtt.host, "broker.local");
    assert_eq!(mqtt.protocol, MqttProtocol::V3_1_1);
    assert_eq!(
        db.secrets()
            .mqtt_password()
            .await
            .unwrap()
            .unwrap()
            .expose_secret(),
        "mqtt-secret"
    );
    assert_eq!(
        db.settings().get::<u32>("stale_multiplier").await.unwrap(),
        Some(3)
    );
    db.close().await;
}

#[tokio::test]
async fn wrong_master_key_is_a_clear_error_at_open() {
    let dir = tempfile::tempdir().unwrap();
    {
        let db = Database::open(dir.path(), key(1)).await.unwrap();
        db.accounts()
            .insert(new_account("a@example.test"), &Secret::new("pw".into()))
            .await
            .unwrap();
        db.close().await;
    }
    let result = Database::open(dir.path(), key(2)).await;
    assert!(
        matches!(result, Err(PersistenceError::MasterKeyMismatch)),
        "{result:?}"
    );
}

#[tokio::test]
async fn swapped_ciphertexts_fail_authentication() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path(), key(1)).await.unwrap();
    let first = db
        .accounts()
        .insert(
            new_account("first@example.test"),
            &Secret::new("first-pw".into()),
        )
        .await
        .unwrap();
    let second = db
        .accounts()
        .insert(
            new_account("second@example.test"),
            &Secret::new("second-pw".into()),
        )
        .await
        .unwrap();

    let pool = raw_pool(dir.path()).await;
    let row = sqlx::query("SELECT nonce, ciphertext FROM secrets WHERE owner_id = ?")
        .bind(second.id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    let nonce: Vec<u8> = row.try_get("nonce").unwrap();
    let ciphertext: Vec<u8> = row.try_get("ciphertext").unwrap();
    sqlx::query("UPDATE secrets SET nonce = ?, ciphertext = ? WHERE owner_id = ?")
        .bind(nonce)
        .bind(ciphertext)
        .bind(first.id.as_str())
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    let result = db.secrets().account_password(&first.id).await;
    assert!(
        matches!(result, Err(PersistenceError::Decrypt { .. })),
        "{result:?}"
    );
    assert_eq!(
        db.secrets()
            .account_password(&second.id)
            .await
            .unwrap()
            .unwrap()
            .expose_secret(),
        "second-pw"
    );
    db.close().await;
}

#[tokio::test]
async fn raw_database_file_contains_no_plaintext_secret() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path(), key(1)).await.unwrap();
    let account = db
        .accounts()
        .insert(
            new_account("scan@example.test"),
            &Secret::new(PASSWORD_MARKER.into()),
        )
        .await
        .unwrap();
    db.secrets()
        .set_account_session(&account.id, SESSION_MARKER.as_bytes())
        .await
        .unwrap();
    db.secrets()
        .set_mqtt_password(&Secret::new(PASSWORD_MARKER.into()))
        .await
        .unwrap();
    db.close().await;

    let pool = raw_pool(dir.path()).await;
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;

    for name in ["vag2mqtt.db", "vag2mqtt.db-wal"] {
        let path = dir.path().join(name);
        if !path.exists() {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            find(&bytes, PASSWORD_MARKER.as_bytes()).is_none(),
            "{name} contains the password"
        );
        assert!(
            find(&bytes, SESSION_MARKER.as_bytes()).is_none(),
            "{name} contains the session"
        );
    }
}

#[tokio::test]
async fn deleting_an_account_removes_vehicles_states_and_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path(), key(1)).await.unwrap();
    let account = db
        .accounts()
        .insert(new_account("del@example.test"), &Secret::new("pw".into()))
        .await
        .unwrap();
    let vehicle = db
        .vehicles()
        .upsert_discovered(&account.id, &discovered("0TEST"))
        .await
        .unwrap();
    db.secrets()
        .set_account_session(&account.id, b"session")
        .await
        .unwrap();
    db.vehicle_states()
        .upsert(&vehicle.vin, &sample_state(), &BTreeMap::new())
        .await
        .unwrap();

    db.accounts().delete(&account.id).await.unwrap();

    assert!(db.accounts().get(&account.id).await.unwrap().is_none());
    assert!(db.vehicles().get(&vehicle.vin).await.unwrap().is_none());
    assert!(
        db.vehicle_states()
            .get(&vehicle.vin)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db.secrets()
            .account_password(&account.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        db.secrets()
            .account_session(&account.id)
            .await
            .unwrap()
            .is_none()
    );
    let pool = raw_pool(dir.path()).await;
    let secrets: i64 = sqlx::query("SELECT COUNT(*) AS n FROM secrets")
        .fetch_one(&pool)
        .await
        .unwrap()
        .try_get("n")
        .unwrap();
    assert_eq!(secrets, 0);
    pool.close().await;
    assert!(matches!(
        db.accounts().delete(&account.id).await,
        Err(PersistenceError::NotFound { entity: "account" })
    ));
    db.close().await;
}

#[tokio::test]
async fn rediscovery_keeps_user_settings_and_clears_missing() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path(), key(1)).await.unwrap();
    let account = db
        .accounts()
        .insert(new_account("re@example.test"), &Secret::new("pw".into()))
        .await
        .unwrap();
    let first = db
        .vehicles()
        .upsert_discovered(&account.id, &discovered("0TEST"))
        .await
        .unwrap();
    assert!(first.enabled);
    assert_eq!(first.data_state, VehicleDataState::Pending);

    db.vehicles()
        .set_display_name(&first.vin, Some("Mine"))
        .await
        .unwrap();
    db.vehicles().set_enabled(&first.vin, false).await.unwrap();
    let since = Utc.with_ymd_and_hms(2026, 9, 22, 8, 0, 0).single().unwrap();
    db.vehicles().mark_missing(&first.vin, since).await.unwrap();
    let later = since + chrono::Duration::hours(1);
    db.vehicles().mark_missing(&first.vin, later).await.unwrap();
    assert_eq!(
        db.vehicles()
            .get(&first.vin)
            .await
            .unwrap()
            .unwrap()
            .missing_since,
        Some(since)
    );

    let again = db
        .vehicles()
        .upsert_discovered(
            &account.id,
            &DiscoveredVehicle {
                vin: vin("0TEST"),
                model: Some("A6 e-tron quattro".to_string()),
                drivetrain: Drivetrain::Electric,
            },
        )
        .await
        .unwrap();
    assert_eq!(again.display_name.as_deref(), Some("Mine"));
    assert!(!again.enabled);
    assert_eq!(again.model.as_deref(), Some("A6 e-tron quattro"));
    assert_eq!(again.missing_since, None);
    db.close().await;
}

#[tokio::test]
async fn a_vin_belongs_to_exactly_one_account() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path(), key(1)).await.unwrap();
    let first = db
        .accounts()
        .insert(new_account("one@example.test"), &Secret::new("pw".into()))
        .await
        .unwrap();
    let second = db
        .accounts()
        .insert(new_account("two@example.test"), &Secret::new("pw".into()))
        .await
        .unwrap();
    db.vehicles()
        .upsert_discovered(&first.id, &discovered("0TEST"))
        .await
        .unwrap();
    let result = db
        .vehicles()
        .upsert_discovered(&second.id, &discovered("0TEST"))
        .await;
    assert!(
        matches!(result, Err(PersistenceError::VinOwnedByOtherAccount { ref vin_short }) if vin_short == "…TEST"),
        "{result:?}"
    );
    let owner = db.vehicles().get(&vin("0TEST")).await.unwrap().unwrap();
    assert_eq!(owner.account_id, first.id);
    db.close().await;
}

#[tokio::test]
async fn runtime_status_and_config_updates_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let db = Database::open(dir.path(), key(1)).await.unwrap();
    let account = db
        .accounts()
        .insert(new_account("st@example.test"), &Secret::new("pw".into()))
        .await
        .unwrap();
    let vehicle = db
        .vehicles()
        .upsert_discovered(&account.id, &discovered("0TEST"))
        .await
        .unwrap();

    let at = Utc
        .with_ymd_and_hms(2026, 9, 22, 9, 30, 0)
        .single()
        .unwrap();
    let error = LastError {
        at,
        category: ErrorCategory::RateLimit,
        message: "throttled".to_string(),
    };
    db.accounts()
        .update_runtime_status(
            &account.id,
            AccountConnectionState::RateLimited,
            Some(at),
            Some(&error),
        )
        .await
        .unwrap();
    db.vehicles()
        .update_runtime_status(&vehicle.vin, VehicleDataState::Stale, Some(at), None)
        .await
        .unwrap();
    db.accounts()
        .update_config(
            &account.id,
            AccountConfigPatch {
                username: None,
                polling: Some(PollingConfig {
                    interval: Duration::from_secs(120),
                }),
            },
        )
        .await
        .unwrap();

    let account = db.accounts().get(&account.id).await.unwrap().unwrap();
    assert_eq!(
        account.connection_state,
        AccountConnectionState::RateLimited
    );
    assert_eq!(account.last_success_at, Some(at));
    assert_eq!(account.last_error, Some(error));
    assert_eq!(account.polling.interval, Duration::from_secs(120));
    assert_eq!(account.username, "st@example.test");
    let vehicle = db.vehicles().get(&vehicle.vin).await.unwrap().unwrap();
    assert_eq!(vehicle.data_state, VehicleDataState::Stale);
    assert_eq!(vehicle.last_update_at, Some(at));
    assert_eq!(vehicle.last_error, None);
    db.close().await;
}

#[tokio::test]
async fn provided_hex_key_is_accepted_and_validated() {
    let dir = tempfile::tempdir().unwrap();
    let hex_key = "ab".repeat(32);
    let db = Database::open(dir.path(), MasterKeySource::Provided(Secret::new(hex_key)))
        .await
        .unwrap();
    assert_eq!(db.key_file_path(), None);
    assert!(!dir.path().join("master.key").exists());
    db.close().await;

    let result = Database::open(
        dir.path(),
        MasterKeySource::Provided(Secret::new("not-hex".to_string())),
    )
    .await;
    assert!(matches!(
        result,
        Err(PersistenceError::MasterKeyInvalid { .. })
    ));
}

async fn raw_pool(dir: &std::path::Path) -> sqlx::SqlitePool {
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(dir.join("vag2mqtt.db"));
    sqlx::SqlitePool::connect_with(options).await.unwrap()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}
