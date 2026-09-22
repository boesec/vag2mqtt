//! Encrypted secrets: account passwords, connector sessions, the MQTT password.

use sqlx::{Row, SqliteConnection};
use vag2mqtt_domain::{AccountId, Secret};
use zeroize::Zeroizing;

use crate::crypto::{EncryptedBlob, NONCE_LEN};
use crate::database::Database;
use crate::error::PersistenceError;
use crate::rows;

/// Who a secret belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Owner {
    Account(AccountId),
    Mqtt,
}

impl Owner {
    fn kind(&self) -> &'static str {
        match self {
            Owner::Account(_) => "account",
            Owner::Mqtt => "mqtt",
        }
    }

    fn id(&self) -> &str {
        match self {
            Owner::Account(id) => id.as_str(),
            Owner::Mqtt => "default",
        }
    }

    fn describe(&self) -> String {
        match self {
            Owner::Account(id) => format!("account {id}"),
            Owner::Mqtt => "mqtt".to_string(),
        }
    }
}

/// What kind of secret it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Password,
    Session,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Password => "password",
            Kind::Session => "session",
        }
    }
}

fn aad(owner: &Owner, kind: Kind) -> Vec<u8> {
    let mut aad = Vec::new();
    aad.extend_from_slice(owner.kind().as_bytes());
    aad.push(0);
    aad.extend_from_slice(owner.id().as_bytes());
    aad.push(0);
    aad.extend_from_slice(kind.as_str().as_bytes());
    aad
}

/// Encrypted secrets, bound to their owner row through associated data.
pub struct Secrets<'a> {
    db: &'a Database,
}

impl<'a> Secrets<'a> {
    pub(crate) fn new(db: &'a Database) -> Self {
        Self { db }
    }

    /// The account's login password.
    pub async fn account_password(
        &self,
        id: &AccountId,
    ) -> Result<Option<Secret<String>>, PersistenceError> {
        self.get_string(&Owner::Account(id.clone()), Kind::Password)
            .await
    }

    /// Stores the account's login password.
    pub async fn set_account_password(
        &self,
        id: &AccountId,
        password: &Secret<String>,
    ) -> Result<(), PersistenceError> {
        let mut conn = self.db.pool().acquire().await?;
        self.store(
            &mut conn,
            &Owner::Account(id.clone()),
            Kind::Password,
            password.expose_secret().as_bytes(),
        )
        .await
    }

    /// The connector session for the account, as opaque bytes.
    pub async fn account_session(
        &self,
        id: &AccountId,
    ) -> Result<Option<Secret<Vec<u8>>>, PersistenceError> {
        self.get_bytes(&Owner::Account(id.clone()), Kind::Session)
            .await
    }

    /// Stores the connector session for the account.
    pub async fn set_account_session(
        &self,
        id: &AccountId,
        session: &[u8],
    ) -> Result<(), PersistenceError> {
        let mut conn = self.db.pool().acquire().await?;
        self.store(
            &mut conn,
            &Owner::Account(id.clone()),
            Kind::Session,
            session,
        )
        .await
    }

    /// Forgets the connector session, forcing a new login next time.
    pub async fn delete_account_session(&self, id: &AccountId) -> Result<(), PersistenceError> {
        let mut conn = self.db.pool().acquire().await?;
        delete_one(&mut conn, &Owner::Account(id.clone()), Kind::Session).await
    }

    /// The MQTT broker password.
    pub async fn mqtt_password(&self) -> Result<Option<Secret<String>>, PersistenceError> {
        self.get_string(&Owner::Mqtt, Kind::Password).await
    }

    /// Stores the MQTT broker password.
    pub async fn set_mqtt_password(
        &self,
        password: &Secret<String>,
    ) -> Result<(), PersistenceError> {
        let mut conn = self.db.pool().acquire().await?;
        self.store(
            &mut conn,
            &Owner::Mqtt,
            Kind::Password,
            password.expose_secret().as_bytes(),
        )
        .await
    }

    /// Removes the MQTT broker password.
    pub async fn delete_mqtt_password(&self) -> Result<(), PersistenceError> {
        let mut conn = self.db.pool().acquire().await?;
        delete_one(&mut conn, &Owner::Mqtt, Kind::Password).await
    }

    async fn get_string(
        &self,
        owner: &Owner,
        kind: Kind,
    ) -> Result<Option<Secret<String>>, PersistenceError> {
        let bytes = self.get_bytes(owner, kind).await?;
        bytes
            .map(|bytes| {
                String::from_utf8(bytes.expose_secret().clone())
                    .map(Secret::new)
                    .map_err(|_| PersistenceError::InvalidRow {
                        table: "secrets",
                        reason: format!("{}: not UTF-8", kind.as_str()),
                    })
            })
            .transpose()
    }

    async fn get_bytes(
        &self,
        owner: &Owner,
        kind: Kind,
    ) -> Result<Option<Secret<Vec<u8>>>, PersistenceError> {
        let row = sqlx::query(
            "SELECT key_version, nonce, ciphertext FROM secrets \
             WHERE owner_kind = ? AND owner_id = ? AND kind = ?",
        )
        .bind(owner.kind())
        .bind(owner.id())
        .bind(kind.as_str())
        .fetch_optional(self.db.pool())
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let key_version: i64 = row.try_get("key_version")?;
        let nonce: Vec<u8> = row.try_get("nonce")?;
        let ciphertext: Vec<u8> = row.try_get("ciphertext")?;
        let nonce: [u8; NONCE_LEN] =
            nonce
                .as_slice()
                .try_into()
                .map_err(|_| PersistenceError::InvalidRow {
                    table: "secrets",
                    reason: "nonce length".to_string(),
                })?;
        let blob = EncryptedBlob {
            key_version: u32::try_from(key_version).map_err(|_| PersistenceError::InvalidRow {
                table: "secrets",
                reason: "key_version".to_string(),
            })?,
            nonce,
            ciphertext,
        };
        let plain: Zeroizing<Vec<u8>> = self
            .db
            .cipher()
            .decrypt(&aad(owner, kind), &blob)
            .map_err(|_| PersistenceError::Decrypt {
                owner: owner.describe(),
            })?;
        Ok(Some(Secret::new(plain.to_vec())))
    }

    pub(crate) async fn store(
        &self,
        conn: &mut SqliteConnection,
        owner: &Owner,
        kind: Kind,
        plaintext: &[u8],
    ) -> Result<(), PersistenceError> {
        let blob = self.db.cipher().encrypt(&aad(owner, kind), plaintext);
        sqlx::query(
            "INSERT INTO secrets (owner_kind, owner_id, kind, key_version, nonce, ciphertext, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(owner_kind, owner_id, kind) DO UPDATE SET \
             key_version = excluded.key_version, nonce = excluded.nonce, \
             ciphertext = excluded.ciphertext, updated_at = excluded.updated_at",
        )
        .bind(owner.kind())
        .bind(owner.id())
        .bind(kind.as_str())
        .bind(i64::from(blob.key_version))
        .bind(blob.nonce.to_vec())
        .bind(blob.ciphertext)
        .bind(rows::now())
        .execute(conn)
        .await?;
        Ok(())
    }
}

async fn delete_one(
    conn: &mut SqliteConnection,
    owner: &Owner,
    kind: Kind,
) -> Result<(), PersistenceError> {
    sqlx::query("DELETE FROM secrets WHERE owner_kind = ? AND owner_id = ? AND kind = ?")
        .bind(owner.kind())
        .bind(owner.id())
        .bind(kind.as_str())
        .execute(conn)
        .await?;
    Ok(())
}

/// Deletes every secret of an owner. Used inside delete transactions.
pub(crate) async fn delete_all_for(
    conn: &mut SqliteConnection,
    owner: &Owner,
) -> Result<(), PersistenceError> {
    sqlx::query("DELETE FROM secrets WHERE owner_kind = ? AND owner_id = ?")
        .bind(owner.kind())
        .bind(owner.id())
        .execute(conn)
        .await?;
    Ok(())
}
