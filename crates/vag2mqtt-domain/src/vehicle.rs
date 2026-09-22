//! Vehicle: discovered through a manufacturer service and assigned to an account (requirements 4.2).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::LastError;
use crate::id::{AccountId, Brand, Drivetrain, Vin};

/// A vehicle's identity, configuration and status, without its measured state.
///
/// The measured state is a separate [`VehicleState`](crate::VehicleState) snapshot, because it
/// changes with every poll while this record changes only on discovery or user edits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vehicle {
    /// The vehicle identification number, the primary identifier on MQTT.
    pub vin: Vin,
    /// The account this vehicle was discovered through.
    pub account_id: AccountId,
    /// The vehicle's brand.
    pub brand: Brand,
    /// The manufacturer's model designation, if delivered.
    pub model: Option<String>,
    /// A user editable name. `None` means "use the model, or failing that the VIN".
    pub display_name: Option<String>,
    /// The propulsion, derived from the manufacturer's car type.
    pub drivetrain: Drivetrain,
    /// Whether the runtime should poll this vehicle. Disabling a vehicle keeps its account.
    pub enabled: bool,
    /// Freshness of the vehicle's data as judged by the runtime.
    pub data_state: VehicleDataState,
    /// When the vehicle's state was last fetched successfully.
    pub last_update_at: Option<DateTime<Utc>>,
    /// The last error, if any, for diagnostics.
    pub last_error: Option<LastError>,
}

impl Vehicle {
    /// The name to show: the display name, else the model, else the VIN.
    pub fn label(&self) -> &str {
        self.display_name
            .as_deref()
            .or(self.model.as_deref())
            .unwrap_or_else(|| self.vin.as_str())
    }
}

/// The data state of a vehicle (FR-018, vehicle availability).
///
/// `Fresh` versus `Stale` is decided by the runtime from the age of the last update and the
/// polling interval; the domain only stores the verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VehicleDataState {
    /// Not fetched since the service started.
    Pending,
    /// The last fetch succeeded recently.
    Fresh,
    /// The last successful fetch is older than the configured multiple of the polling interval.
    Stale,
    /// The account works but this vehicle delivers no data.
    Unavailable,
    /// Fetching this vehicle keeps failing.
    Error,
    /// The vehicle is disabled by the user.
    Disabled,
}

impl VehicleDataState {
    /// Every variant, in declaration order.
    pub const ALL: &'static [VehicleDataState] = &[
        VehicleDataState::Pending,
        VehicleDataState::Fresh,
        VehicleDataState::Stale,
        VehicleDataState::Unavailable,
        VehicleDataState::Error,
        VehicleDataState::Disabled,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vehicle() -> Vehicle {
        Vehicle {
            vin: Vin::new("WAUZZZ0000000TEST").unwrap(),
            account_id: AccountId::new("acc-1").unwrap(),
            brand: Brand::Audi,
            model: None,
            display_name: None,
            drivetrain: Drivetrain::Electric,
            enabled: true,
            data_state: VehicleDataState::Pending,
            last_update_at: None,
            last_error: None,
        }
    }

    #[test]
    fn label_falls_back_from_display_name_to_model_to_vin() {
        let mut vehicle = vehicle();
        assert_eq!(vehicle.label(), "WAUZZZ0000000TEST");
        vehicle.model = Some("A6 e-tron".into());
        assert_eq!(vehicle.label(), "A6 e-tron");
        vehicle.display_name = Some("Family car".into());
        assert_eq!(vehicle.label(), "Family car");
    }

    #[test]
    fn data_state_round_trips() {
        for state in VehicleDataState::ALL {
            let json = serde_json::to_string(state).unwrap();
            let back: VehicleDataState = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, state);
        }
    }
}
