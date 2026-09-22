//! Authenticated encryption of stored secrets.

use chacha20poly1305::aead::{Aead, OsRng, Payload};
use chacha20poly1305::{AeadCore, ChaCha20Poly1305, Key, KeyInit, Nonce};
use zeroize::Zeroizing;

use crate::master_key::MasterKey;

/// Size of a ChaCha20-Poly1305 nonce in bytes.
pub(crate) const NONCE_LEN: usize = 12;

/// The cipher bound to the current master key.
pub(crate) struct Cipher {
    version: u32,
    aead: ChaCha20Poly1305,
}

/// One encrypted value as stored in the `secrets` table.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct EncryptedBlob {
    pub(crate) key_version: u32,
    pub(crate) nonce: [u8; NONCE_LEN],
    pub(crate) ciphertext: Vec<u8>,
}

/// Why decryption failed. Deliberately without detail: the caller knows the owner.
#[derive(Debug)]
pub(crate) struct DecryptError;

impl Cipher {
    pub(crate) fn new(key: &MasterKey) -> Self {
        Self {
            version: key.version,
            aead: ChaCha20Poly1305::new(Key::from_slice(key.bytes.expose_secret())),
        }
    }

    /// Encrypts `plaintext` bound to `aad` under a fresh random nonce.
    pub(crate) fn encrypt(&self, aad: &[u8], plaintext: &[u8]) -> EncryptedBlob {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ciphertext = self
            .aead
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad,
                },
            )
            .unwrap_or_else(|_| {
                // ChaCha20-Poly1305 encryption cannot fail for inputs that fit in memory.
                unreachable!("ChaCha20-Poly1305 encryption failed")
            });
        EncryptedBlob {
            key_version: self.version,
            nonce: nonce.into(),
            ciphertext,
        }
    }

    /// Decrypts a blob bound to `aad`. Fails when the key, the nonce, the associated data or the
    /// ciphertext do not match.
    pub(crate) fn decrypt(
        &self,
        aad: &[u8],
        blob: &EncryptedBlob,
    ) -> Result<Zeroizing<Vec<u8>>, DecryptError> {
        if blob.key_version != self.version {
            return Err(DecryptError);
        }
        let nonce = Nonce::from_slice(&blob.nonce);
        self.aead
            .decrypt(
                nonce,
                Payload {
                    msg: &blob.ciphertext,
                    aad,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| DecryptError)
    }
}

#[cfg(test)]
mod tests {
    use vag2mqtt_domain::Secret;

    use super::*;

    fn cipher_with(byte: u8) -> Cipher {
        Cipher::new(&MasterKey {
            version: 1,
            bytes: Secret::new([byte; 32]),
            file: None,
        })
    }

    #[test]
    fn round_trip_with_matching_aad() {
        let cipher = cipher_with(7);
        let blob = cipher.encrypt(b"account\0id\0password", b"hunter2");
        let plain = cipher.decrypt(b"account\0id\0password", &blob).unwrap();
        assert_eq!(plain.as_slice(), b"hunter2");
        assert_ne!(blob.ciphertext, b"hunter2");
    }

    #[test]
    fn wrong_aad_or_key_fails() {
        let cipher = cipher_with(7);
        let blob = cipher.encrypt(b"a", b"hunter2");
        assert!(cipher.decrypt(b"b", &blob).is_err());
        assert!(cipher_with(8).decrypt(b"a", &blob).is_err());
    }

    #[test]
    fn nonces_differ_per_encryption() {
        let cipher = cipher_with(7);
        let first = cipher.encrypt(b"a", b"x");
        let second = cipher.encrypt(b"a", b"x");
        assert_ne!(first.nonce, second.nonce);
        assert_ne!(first.ciphertext, second.ciphertext);
    }
}
