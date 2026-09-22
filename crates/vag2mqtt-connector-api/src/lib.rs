//! The interface every brand connector implements.
//!
//! Connectors are compiled into the binary and selected through a static registry; there
//! is deliberately no plugin system (requirements section 3 and AD-006). This crate owns
//! the connector trait, the serialisable session state, the data source kind from
//! requirements section 4.4 and the error classification the supervisor uses to decide
//! whether to retry.
//!
//! Implemented by WP-03.
