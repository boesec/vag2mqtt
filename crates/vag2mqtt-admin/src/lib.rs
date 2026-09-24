//! Management API and interface for VAG2MQTT.
//!
//! A translation layer and nothing more: HTTP in, a [`RuntimeHandle`](vag2mqtt_runtime::RuntimeHandle)
//! call, HTTP out. Every rule lives in the runtime, where it is already tested. Reads come from
//! the runtime's status snapshot, so they never touch the database.
//!
//! The surface is described by `Docs/openapi.yaml`, served at `/api/openapi.yaml`. A test keeps
//! the document and the router honest by comparing both against [`api::ROUTES`], the single list
//! the router is built from.
//!
//! Implemented by WP-08. The htmx interface on top of it is WP-09.

pub mod api;
pub mod dto;
mod error;
mod state;
mod ui;

pub use api::{ROUTES, RouteSpec};
pub use error::{ApiError, ErrorBody};
pub use state::AppState;
pub use state::COMMAND_TIMEOUT;

use std::net::SocketAddr;

use tokio::net::TcpListener;

/// Binds and serves the API until `shutdown` resolves.
///
/// Logs one warning when bound to anything other than a loopback address, because the API has no
/// authentication yet (WP-23) and a wide bind should be visible rather than silent.
pub async fn serve(
    listen: SocketAddr,
    state: AppState,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    warn_if_exposed(listen);
    let listener = TcpListener::bind(listen).await?;
    let bound = listener.local_addr()?;
    tracing::info!(target: "vag2mqtt::admin", address = %bound, "management interface listening");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown)
        .await
}

/// The whole surface: the JSON API and the interface that renders it.
///
/// Both are one axum router, because the interface is served from the same origin and calls the
/// runtime through the same handle. Keeping them apart would only add a network hop to itself.
pub fn router(state: AppState) -> axum::Router {
    api::router(state.clone()).merge(ui::router().with_state(state))
}

/// Warns once when the API is reachable from outside this machine.
pub fn warn_if_exposed(listen: SocketAddr) {
    if !listen.ip().is_loopback() {
        tracing::warn!(
            target: "vag2mqtt::admin",
            address = %listen,
            "the management interface is bound to a non loopback address and has no authentication; \
             anyone who can reach this port can read and change the configuration"
        );
    }
}
