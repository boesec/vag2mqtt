//! Vehicle status: connectivity, activity and environment.

use serde::{Deserialize, Serialize};

use crate::impl_unitless;
use crate::reading::Reading;
use crate::units::Celsius;

/// The vehicle's own status, as opposed to one of its subsystems.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VehicleStatus {
    /// Whether the vehicle is reachable by the manufacturer backend.
    pub connection: Reading<VehicleConnection>,
    /// What the vehicle is doing.
    pub activity: Reading<VehicleActivity>,
    /// Whether the vehicle is secured (locked, everything closed).
    pub secured: Reading<bool>,
    /// The outside temperature the vehicle measures.
    pub outside_temperature: Reading<Celsius>,
}

impl VehicleStatus {
    /// Every value unsupported.
    pub fn unsupported() -> Self {
        Self {
            connection: Reading::Unsupported,
            activity: Reading::Unsupported,
            secured: Reading::Unsupported,
            outside_temperature: Reading::Unsupported,
        }
    }
}

/// Whether the manufacturer backend can reach the vehicle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VehicleConnection {
    /// The vehicle is online.
    Online,
    /// The vehicle is offline; data is from the last contact.
    Offline,
    /// A manufacturer value we cannot name.
    Unknown,
}

impl VehicleConnection {
    /// Every variant, in declaration order.
    pub const ALL: &'static [VehicleConnection] = &[
        VehicleConnection::Online,
        VehicleConnection::Offline,
        VehicleConnection::Unknown,
    ];
}

/// What the vehicle is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VehicleActivity {
    /// Parked.
    Parked,
    /// Ignition on, not moving.
    IgnitionOn,
    /// Driving.
    Driving,
    /// A manufacturer value we cannot name.
    Unknown,
}

impl VehicleActivity {
    /// Every variant, in declaration order.
    pub const ALL: &'static [VehicleActivity] = &[
        VehicleActivity::Parked,
        VehicleActivity::IgnitionOn,
        VehicleActivity::Driving,
        VehicleActivity::Unknown,
    ];
}

impl_unitless!(VehicleConnection, VehicleActivity);
