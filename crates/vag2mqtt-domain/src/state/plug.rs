//! The external power connector.

use serde::{Deserialize, Serialize};

use super::access::LockState;
use crate::impl_unitless;
use crate::reading::Reading;

/// The charging plug and the power behind it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Plug {
    /// Whether a cable is plugged in.
    pub connection: Reading<PlugConnection>,
    /// Whether the plug is locked in the socket.
    pub lock: Reading<LockState>,
    /// Whether the charger offers power.
    pub external_power: Reading<ExternalPower>,
}

impl Plug {
    /// Every value unsupported, for vehicles without a charging socket.
    pub fn unsupported() -> Self {
        Self {
            connection: Reading::Unsupported,
            lock: Reading::Unsupported,
            external_power: Reading::Unsupported,
        }
    }
}

/// Whether a charging cable is plugged in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlugConnection {
    /// A cable is connected.
    Connected,
    /// No cable is connected.
    Disconnected,
    /// A manufacturer value we cannot name.
    Unknown,
}

impl PlugConnection {
    /// Every variant, in declaration order.
    pub const ALL: &'static [PlugConnection] = &[
        PlugConnection::Connected,
        PlugConnection::Disconnected,
        PlugConnection::Unknown,
    ];
}

/// Whether the charger behind the plug offers power.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalPower {
    /// Power is offered but not drawn.
    Available,
    /// Power is being drawn.
    Active,
    /// No power is offered.
    Unavailable,
    /// A manufacturer value we cannot name.
    Unknown,
}

impl ExternalPower {
    /// Every variant, in declaration order.
    pub const ALL: &'static [ExternalPower] = &[
        ExternalPower::Available,
        ExternalPower::Active,
        ExternalPower::Unavailable,
        ExternalPower::Unknown,
    ];
}

impl_unitless!(PlugConnection, ExternalPower);
