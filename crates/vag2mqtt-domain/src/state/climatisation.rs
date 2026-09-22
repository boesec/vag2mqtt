//! Climatisation.

use serde::{Deserialize, Serialize};

use crate::impl_unitless;
use crate::reading::Reading;
use crate::units::{Celsius, Minutes};

/// Cabin climatisation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Climatisation {
    /// What the climatisation is doing.
    pub state: Reading<ClimatisationState>,
    /// The temperature it aims for.
    pub target_temperature: Reading<Celsius>,
    /// How long it will keep running.
    pub remaining_time: Reading<Minutes>,
}

impl Climatisation {
    /// Every value unsupported, for vehicles without remote climatisation.
    pub fn unsupported() -> Self {
        Self {
            state: Reading::Unsupported,
            target_temperature: Reading::Unsupported,
            remaining_time: Reading::Unsupported,
        }
    }
}

/// What the climatisation is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClimatisationState {
    /// Off.
    Off,
    /// Heating the cabin.
    Heating,
    /// Cooling the cabin.
    Cooling,
    /// Ventilating without heating or cooling.
    Ventilation,
    /// A manufacturer value we cannot name.
    Unknown,
}

impl ClimatisationState {
    /// Every variant, in declaration order.
    pub const ALL: &'static [ClimatisationState] = &[
        ClimatisationState::Off,
        ClimatisationState::Heating,
        ClimatisationState::Cooling,
        ClimatisationState::Ventilation,
        ClimatisationState::Unknown,
    ];
}

impl_unitless!(ClimatisationState);
