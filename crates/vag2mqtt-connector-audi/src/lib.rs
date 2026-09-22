//! Audi connector: authentication, vehicle discovery and status retrieval.
//!
//! This is the only crate that knows Audi endpoints, headers and response shapes. It
//! normalises everything into [`vag2mqtt_domain`] types before handing data out, so no
//! Audi specific type ever leaves this crate (requirement DR-001).
//!
//! The endpoints and OAuth parameters used here are documented with their source in
//! `Docs/reference/audi-auth.md` and `Docs/reference/audi-api.md`.
//!
//! Implemented by WP-06 (authentication) and WP-07 (vehicle data).
