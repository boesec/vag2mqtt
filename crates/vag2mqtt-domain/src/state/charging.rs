//! Charging process and settings.

use serde::{Deserialize, Serialize};

use crate::impl_unitless;
use crate::reading::Reading;
use crate::units::{Kilowatts, Minutes, Percent};

/// The charging process and its settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Charging {
    /// What the charger is doing.
    pub state: Reading<ChargingState>,
    /// How charging is scheduled.
    pub mode: Reading<ChargingMode>,
    /// AC or DC.
    pub current_type: Reading<ChargeCurrentType>,
    /// Current charging power.
    pub power: Reading<Kilowatts>,
    /// Time until the target state of charge is reached.
    pub remaining_time: Reading<Minutes>,
    /// The state of charge the vehicle charges to.
    pub target_soc: Reading<Percent>,
}

impl Charging {
    /// Every value unsupported, for vehicles that cannot charge.
    pub fn unsupported() -> Self {
        Self {
            state: Reading::Unsupported,
            mode: Reading::Unsupported,
            current_type: Reading::Unsupported,
            power: Reading::Unsupported,
            remaining_time: Reading::Unsupported,
            target_soc: Reading::Unsupported,
        }
    }
}

/// What the charger is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargingState {
    /// Not charging and not ready to.
    Off,
    /// Plugged in and waiting, for example for a timer or for the target to drop below the level.
    ReadyForCharging,
    /// Energy is flowing into the battery.
    Charging,
    /// Target reached; the charger keeps the level (trickle or conservation charging).
    Conservation,
    /// Energy is flowing out of the battery (vehicle to grid or home).
    Discharging,
    /// The vehicle reports a charging error.
    Error,
    /// A manufacturer state we cannot name.
    Unknown,
}

impl ChargingState {
    /// Every variant, in declaration order.
    pub const ALL: &'static [ChargingState] = &[
        ChargingState::Off,
        ChargingState::ReadyForCharging,
        ChargingState::Charging,
        ChargingState::Conservation,
        ChargingState::Discharging,
        ChargingState::Error,
        ChargingState::Unknown,
    ];
}

/// How charging is scheduled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargingMode {
    /// Charge as soon as plugged in.
    Manual,
    /// Charge according to departure timers.
    Timer,
    /// Charge within preferred time windows.
    PreferredTimes,
    /// Scheduled charging is off.
    Off,
    /// A manufacturer mode we cannot name.
    Unknown,
}

impl ChargingMode {
    /// Every variant, in declaration order.
    pub const ALL: &'static [ChargingMode] = &[
        ChargingMode::Manual,
        ChargingMode::Timer,
        ChargingMode::PreferredTimes,
        ChargingMode::Off,
        ChargingMode::Unknown,
    ];
}

/// The kind of current at the connector.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChargeCurrentType {
    /// Alternating current (wallbox, household socket).
    Ac,
    /// Direct current (fast charger).
    Dc,
    /// A manufacturer value we cannot name.
    Unknown,
}

impl ChargeCurrentType {
    /// Every variant, in declaration order.
    pub const ALL: &'static [ChargeCurrentType] = &[
        ChargeCurrentType::Ac,
        ChargeCurrentType::Dc,
        ChargeCurrentType::Unknown,
    ];
}

impl_unitless!(ChargingState, ChargingMode, ChargeCurrentType);
