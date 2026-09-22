//! Traction battery and fuel tank.

use serde::{Deserialize, Serialize};

use crate::reading::Reading;
use crate::units::{Kilometres, Percent};

/// The traction battery of an electric or hybrid vehicle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Battery {
    /// State of charge.
    pub soc: Reading<Percent>,
    /// Remaining electric range.
    pub range: Reading<Kilometres>,
}

impl Battery {
    /// Every value unsupported, for vehicles without a traction battery.
    pub fn unsupported() -> Self {
        Self {
            soc: Reading::Unsupported,
            range: Reading::Unsupported,
        }
    }
}

/// The fuel tank of a combustion or hybrid vehicle.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fuel {
    /// Fuel level.
    pub level: Reading<Percent>,
    /// Remaining range on fuel.
    pub range: Reading<Kilometres>,
}

impl Fuel {
    /// Every value unsupported, for vehicles without a fuel tank.
    pub fn unsupported() -> Self {
        Self {
            level: Reading::Unsupported,
            range: Reading::Unsupported,
        }
    }
}
