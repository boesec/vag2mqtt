//! Brand independent vehicle domain model for VAG2MQTT.
//!
//! This crate holds pure data types: accounts, vehicles, vehicle state, the error
//! categories and the three state value container from section 4.3 of the requirements
//! (unsupported / temporarily unavailable / present).
//!
//! It performs no I/O. It must not depend on `tokio`, `reqwest`, `sqlx` or `rumqttc`;
//! every other crate in the workspace may depend on it.
//!
//! Implemented by WP-01.
