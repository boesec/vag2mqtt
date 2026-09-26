//! Where the master key comes from and how it is stored.

use std::path::{Path, PathBuf};

use chacha20poly1305::aead::OsRng;
use chacha20poly1305::{ChaCha20Poly1305, KeyInit};
use vag2mqtt_domain::Secret;

use crate::error::PersistenceError;

/// Name of the key file inside the data directory.
pub(crate) const KEY_FILE_NAME: &str = "master.key";

/// Where the master key comes from.
pub enum MasterKeySource {
    /// Sixty-four hex characters, typically from `VAG2MQTT_MASTER_KEY`.
    Provided(Secret<String>),
    /// `<data-dir>/master.key`, generated with restrictive permissions if absent.
    KeyFile,
    /// Raw key bytes, for tests.
    Raw(Secret<[u8; 32]>),
}

impl std::fmt::Debug for MasterKeySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MasterKeySource::Provided(_) => f.write_str("MasterKeySource::Provided([REDACTED])"),
            MasterKeySource::KeyFile => f.write_str("MasterKeySource::KeyFile"),
            MasterKeySource::Raw(_) => f.write_str("MasterKeySource::Raw([REDACTED])"),
        }
    }
}

/// The resolved key and where it came from.
pub(crate) struct MasterKey {
    pub(crate) version: u32,
    pub(crate) bytes: Secret<[u8; 32]>,
    pub(crate) file: Option<PathBuf>,
}

/// The only key version in use today.
pub(crate) const CURRENT_KEY_VERSION: u32 = 1;

impl MasterKey {
    pub(crate) fn resolve(
        data_dir: &Path,
        source: MasterKeySource,
    ) -> Result<Self, PersistenceError> {
        match source {
            MasterKeySource::Raw(bytes) => Ok(Self {
                version: CURRENT_KEY_VERSION,
                bytes,
                file: None,
            }),
            MasterKeySource::Provided(hex_key) => Ok(Self {
                version: CURRENT_KEY_VERSION,
                bytes: parse_hex_key(hex_key.expose_secret())?,
                file: None,
            }),
            MasterKeySource::KeyFile => {
                let path = data_dir.join(KEY_FILE_NAME);
                let bytes = if path.exists() {
                    read_key_file(&path)?
                } else {
                    create_key_file(&path)?
                };
                Ok(Self {
                    version: CURRENT_KEY_VERSION,
                    bytes,
                    file: Some(path),
                })
            }
        }
    }
}

fn parse_hex_key(text: &str) -> Result<Secret<[u8; 32]>, PersistenceError> {
    let decoded = hex::decode(text.trim()).map_err(|_| PersistenceError::MasterKeyInvalid {
        reason: "must be hexadecimal",
    })?;
    let bytes: [u8; 32] =
        decoded
            .as_slice()
            .try_into()
            .map_err(|_| PersistenceError::MasterKeyInvalid {
                reason: "must be exactly 32 bytes (64 hex characters)",
            })?;
    Ok(Secret::new(bytes))
}

fn read_key_file(path: &Path) -> Result<Secret<[u8; 32]>, PersistenceError> {
    let text = std::fs::read_to_string(path).map_err(|source| PersistenceError::KeyFile {
        path: path.to_path_buf(),
        source,
    })?;
    parse_hex_key(&text)
}

fn create_key_file(path: &Path) -> Result<Secret<[u8; 32]>, PersistenceError> {
    let key = ChaCha20Poly1305::generate_key(&mut OsRng);
    let bytes: [u8; 32] = key.into();
    let mut line = hex::encode(bytes);
    line.push('\n');
    write_restricted(path, line.as_bytes()).map_err(|source| PersistenceError::KeyFile {
        path: path.to_path_buf(),
        source,
    })?;
    tracing::info!(
        target: "vag2mqtt::persistence",
        path = %path.display(),
        "generated a new master key file; back it up, without it the stored secrets are lost"
    );
    #[cfg(not(unix))]
    tracing::warn!(
        target: "vag2mqtt::persistence",
        path = %path.display(),
        "key file permissions inherit the directory ACL on this platform; restrict them by hand"
    );
    Ok(Secret::new(bytes))
}

#[cfg(unix)]
fn write_restricted(path: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(content)?;
    file.sync_all()
}

#[cfg(not(unix))]
fn write_restricted(path: &Path, content: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(content)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_key_must_be_32_bytes() {
        assert!(matches!(
            parse_hex_key("abcd"),
            Err(PersistenceError::MasterKeyInvalid { .. })
        ));
        assert!(matches!(
            parse_hex_key("zz"),
            Err(PersistenceError::MasterKeyInvalid { .. })
        ));
        let ok = parse_hex_key(&"ab".repeat(32)).unwrap();
        assert_eq!(ok.expose_secret()[0], 0xab);
    }

    #[test]
    fn key_file_is_created_once_and_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let first = MasterKey::resolve(dir.path(), MasterKeySource::KeyFile).unwrap();
        let second = MasterKey::resolve(dir.path(), MasterKeySource::KeyFile).unwrap();
        assert_eq!(first.bytes.expose_secret(), second.bytes.expose_secret());
        assert_eq!(
            first.file.as_deref(),
            Some(dir.path().join(KEY_FILE_NAME).as_path())
        );
    }

    #[cfg(unix)]
    #[test]
    fn key_file_has_mode_0600() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        MasterKey::resolve(dir.path(), MasterKeySource::KeyFile).unwrap();
        let mode = std::fs::metadata(dir.path().join(KEY_FILE_NAME))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn source_debug_is_redacted() {
        let source = MasterKeySource::Provided(Secret::new("ab".repeat(32)));
        assert_eq!(
            format!("{source:?}"),
            "MasterKeySource::Provided([REDACTED])"
        );
    }
}
