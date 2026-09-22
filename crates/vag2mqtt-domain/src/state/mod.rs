//! The normalised vehicle state: one snapshot per fetch, every category from requirements 4.3.

mod access;
mod battery;
mod charging;
mod climatisation;
mod plug;
mod position;
mod service;
mod status;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::reading::Reading;
use crate::units::Kilometres;

pub use access::{DoorPosition, Doors, LockState, OpenState, WindowPosition, Windows};
pub use battery::{Battery, Fuel};
pub use charging::{ChargeCurrentType, Charging, ChargingMode, ChargingState};
pub use climatisation::{Climatisation, ClimatisationState};
pub use plug::{ExternalPower, Plug, PlugConnection};
pub use position::GeoPosition;
pub use service::Service;
pub use status::{VehicleActivity, VehicleConnection, VehicleStatus};

/// Everything VAG2MQTT knows about a vehicle's measured state after one poll.
///
/// Every category exists for every vehicle. A category the vehicle lacks is filled with
/// [`Reading::Unsupported`] values, never omitted, so consumers can rely on the shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VehicleState {
    /// When VAG2MQTT fetched this snapshot. The fallback for `ts` on MQTT when a value has no
    /// manufacturer timestamp of its own (DR-003).
    pub fetched_at: DateTime<Utc>,
    /// Total distance driven.
    pub odometer: Reading<Kilometres>,
    /// Traction battery.
    pub battery: Battery,
    /// Fuel tank.
    pub fuel: Fuel,
    /// Charging process and settings.
    pub charging: Charging,
    /// The external power connector.
    pub plug: Plug,
    /// Doors and the central lock.
    pub doors: Doors,
    /// Windows.
    pub windows: Windows,
    /// Where the vehicle is. Redacted in `Debug`.
    pub position: Reading<GeoPosition>,
    /// Climatisation.
    pub climatisation: Climatisation,
    /// Connectivity, activity and environment of the vehicle.
    pub status: VehicleStatus,
    /// Service and inspection intervals.
    pub service: Service,
}

impl VehicleState {
    /// A snapshot in which every value is [`Reading::Unsupported`].
    ///
    /// The starting point for a connector: it flips to `Unavailable` or `Present` exactly the
    /// categories the vehicle declares, and everything else stays unsupported by construction.
    pub fn unsupported(fetched_at: DateTime<Utc>) -> Self {
        Self {
            fetched_at,
            odometer: Reading::Unsupported,
            battery: Battery::unsupported(),
            fuel: Fuel::unsupported(),
            charging: Charging::unsupported(),
            plug: Plug::unsupported(),
            doors: Doors::unsupported(),
            windows: Windows::unsupported(),
            position: Reading::Unsupported,
            climatisation: Climatisation::unsupported(),
            status: VehicleStatus::unsupported(),
            service: Service::unsupported(),
        }
    }
}
