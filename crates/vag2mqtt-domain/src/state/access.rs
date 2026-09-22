//! Doors, windows and locks.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::impl_unitless;
use crate::reading::Reading;

/// Doors and the central lock.
///
/// `by_position` holds one reading per door the connector knows about. A door the vehicle
/// does not have is simply absent from the map, or present as [`Reading::Unsupported`] if the
/// manufacturer lists it explicitly as unsupported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Doors {
    /// The aggregate: open if any door is open.
    pub overall: Reading<OpenState>,
    /// The central lock.
    pub lock: Reading<LockState>,
    /// Per door.
    pub by_position: BTreeMap<DoorPosition, Reading<OpenState>>,
}

impl Doors {
    /// Every value unsupported and no per door entries.
    pub fn unsupported() -> Self {
        Self {
            overall: Reading::Unsupported,
            lock: Reading::Unsupported,
            by_position: BTreeMap::new(),
        }
    }
}

/// Windows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Windows {
    /// The aggregate: open if any window is open.
    pub overall: Reading<OpenState>,
    /// Per window.
    pub by_position: BTreeMap<WindowPosition, Reading<OpenState>>,
}

impl Windows {
    /// Every value unsupported and no per window entries.
    pub fn unsupported() -> Self {
        Self {
            overall: Reading::Unsupported,
            by_position: BTreeMap::new(),
        }
    }
}

/// Whether something that opens is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenState {
    /// Open.
    Open,
    /// Closed.
    Closed,
    /// Not fully closed.
    Ajar,
    /// A manufacturer value we cannot name.
    Unknown,
}

impl OpenState {
    /// Every variant, in declaration order.
    pub const ALL: &'static [OpenState] = &[
        OpenState::Open,
        OpenState::Closed,
        OpenState::Ajar,
        OpenState::Unknown,
    ];
}

/// Whether a lock is engaged. Used for the central lock and for the charging plug.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockState {
    /// Locked.
    Locked,
    /// Unlocked.
    Unlocked,
    /// A manufacturer value we cannot name.
    Unknown,
}

impl LockState {
    /// Every variant, in declaration order.
    pub const ALL: &'static [LockState] =
        &[LockState::Locked, LockState::Unlocked, LockState::Unknown];
}

/// The doors a vehicle can have.
///
/// A closed set on purpose: a door name the connector does not recognise is dropped and logged
/// at `debug` rather than invented here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoorPosition {
    /// Driver side front on left hand drive vehicles.
    FrontLeft,
    /// Passenger side front on left hand drive vehicles.
    FrontRight,
    /// Rear left.
    RearLeft,
    /// Rear right.
    RearRight,
    /// Boot lid or tailgate.
    Trunk,
    /// Engine or front storage lid.
    Bonnet,
}

impl DoorPosition {
    /// Every variant, in declaration order.
    pub const ALL: &'static [DoorPosition] = &[
        DoorPosition::FrontLeft,
        DoorPosition::FrontRight,
        DoorPosition::RearLeft,
        DoorPosition::RearRight,
        DoorPosition::Trunk,
        DoorPosition::Bonnet,
    ];
}

/// The windows a vehicle can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowPosition {
    /// Front left.
    FrontLeft,
    /// Front right.
    FrontRight,
    /// Rear left.
    RearLeft,
    /// Rear right.
    RearRight,
    /// Sun roof or panorama roof.
    SunRoof,
}

impl WindowPosition {
    /// Every variant, in declaration order.
    pub const ALL: &'static [WindowPosition] = &[
        WindowPosition::FrontLeft,
        WindowPosition::FrontRight,
        WindowPosition::RearLeft,
        WindowPosition::RearRight,
        WindowPosition::SunRoof,
    ];
}

impl_unitless!(OpenState, LockState);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn door_map_keys_serialise_as_snake_case_strings() {
        let mut doors = Doors::unsupported();
        doors.by_position.insert(
            DoorPosition::FrontLeft,
            Reading::present(OpenState::Closed, None),
        );
        doors
            .by_position
            .insert(DoorPosition::Trunk, Reading::Unsupported);
        let json = serde_json::to_value(&doors).unwrap();
        assert_eq!(
            json["by_position"]["front_left"]["value"],
            serde_json::json!("closed")
        );
        assert_eq!(
            json["by_position"]["trunk"]["state"],
            serde_json::json!("unsupported")
        );
        let back: Doors = serde_json::from_value(json).unwrap();
        assert_eq!(back, doors);
    }
}
