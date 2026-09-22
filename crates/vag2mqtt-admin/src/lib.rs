//! Admin API and the embedded management UI.
//!
//! An axum server exposes the JSON API (requirement FR-020) and a server rendered UI built
//! from askama templates plus htmx, with every asset embedded in the binary so the build
//! needs nothing but the Rust toolchain (AD-003, NFR-012).
//!
//! No secret is ever returned by the API or rendered into the UI. Nothing in the workspace
//! depends on this crate.
//!
//! Implemented by WP-08 (API) and WP-09 (UI).
