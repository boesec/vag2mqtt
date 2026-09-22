//! Persistence: SQLite repositories, schema migrations and secret encryption.
//!
//! All state that must survive a restart lives here (requirement FR-010). Credentials and
//! tokens are encrypted with ChaCha20-Poly1305 under a master key taken from the
//! `VAG2MQTT_MASTER_KEY` environment variable or a key file in the data directory
//! (requirement FR-011, AD-005).
//!
//! SQL statements stay inside this crate; callers see repository functions only.
//!
//! Implemented by WP-02.
