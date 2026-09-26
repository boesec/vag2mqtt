//! What every handler needs.

use std::time::Duration;

use vag2mqtt_domain::Brand;
use vag2mqtt_runtime::{RuntimeHandle, RuntimeStatus};

/// How long a write waits for the runtime's confirmation before the API answers 503.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

/// Shared state of the API.
///
/// `brands` is handed in rather than read from a connector registry, because the admin layer must
/// not depend on `vag2mqtt-connector-api` (see the crate layout in CONTRIBUTING.md). The binary knows both and passes
/// the list along.
#[derive(Clone)]
pub struct AppState {
    runtime: RuntimeHandle,
    brands: Vec<Brand>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("brands", &self.brands)
            .finish_non_exhaustive()
    }
}

impl AppState {
    /// Builds the state from the runtime handle and the brands this build ships.
    pub fn new(runtime: RuntimeHandle, brands: Vec<Brand>) -> Self {
        Self { runtime, brands }
    }

    /// The runtime handle.
    pub fn runtime(&self) -> &RuntimeHandle {
        &self.runtime
    }

    /// The brands a connector exists for.
    pub fn brands(&self) -> &[Brand] {
        &self.brands
    }

    /// The latest status snapshot. A clone of the `watch` value, never a database read.
    pub fn status(&self) -> RuntimeStatus {
        self.runtime.current_status()
    }
}
