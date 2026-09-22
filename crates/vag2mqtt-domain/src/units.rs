//! Metric unit newtypes.
//!
//! The unit is part of the type, not a runtime field. A connector that receives miles or
//! Fahrenheit has to convert visibly before it can construct one of these.

use serde::{Deserialize, Serialize};

use crate::error::DomainError;

/// The physical unit of a value, for documentation and the MQTT contract (WP-04).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// km
    Kilometres,
    /// %
    Percent,
    /// kW
    Kilowatts,
    /// °C
    Celsius,
    /// min
    Minutes,
    /// d
    Days,
    /// ° (angles and geographic coordinates)
    Degrees,
    /// Dimensionless: enums, booleans, strings.
    None,
}

impl Unit {
    /// The unit's symbol as consumers expect it, empty for [`Unit::None`].
    pub fn symbol(self) -> &'static str {
        match self {
            Unit::Kilometres => "km",
            Unit::Percent => "%",
            Unit::Kilowatts => "kW",
            Unit::Celsius => "°C",
            Unit::Minutes => "min",
            Unit::Days => "d",
            Unit::Degrees => "°",
            Unit::None => "",
        }
    }
}

/// Implemented by every value type that can appear inside a [`Reading`](crate::Reading),
/// so that the unit of a topic can be stated without inspecting a value.
pub trait HasUnit {
    /// The unit of this type.
    const UNIT: Unit;
}

impl HasUnit for bool {
    const UNIT: Unit = Unit::None;
}

/// Implements [`HasUnit`] for dimensionless types such as enums.
#[macro_export]
macro_rules! impl_unitless {
    ($($ty:ty),+ $(,)?) => {
        $(impl $crate::units::HasUnit for $ty {
            const UNIT: $crate::units::Unit = $crate::units::Unit::None;
        })+
    };
}

macro_rules! unit_newtype {
    (
        $(#[$meta:meta])*
        $name:ident($inner:ty) = $unit:expr, derives = [$($derive:ident),*]
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Serialize, Deserialize $(, $derive)*)]
        #[serde(transparent)]
        pub struct $name($inner);

        impl $name {
            /// Wraps a raw value that is already in the right unit.
            pub const fn new(value: $inner) -> Self {
                Self(value)
            }

            /// The raw value.
            pub const fn value(self) -> $inner {
                self.0
            }
        }

        impl HasUnit for $name {
            const UNIT: Unit = $unit;
        }

        impl From<$inner> for $name {
            fn from(value: $inner) -> Self {
                Self(value)
            }
        }
    };
}

unit_newtype! {
    /// A distance or odometer reading in whole kilometres.
    Kilometres(u32) = Unit::Kilometres, derives = [Eq, Ord, Hash]
}

unit_newtype! {
    /// A service due distance in kilometres; negative when the service is overdue.
    ServiceKilometres(i32) = Unit::Kilometres, derives = [Eq, Ord, Hash]
}

unit_newtype! {
    /// Electrical power in kilowatts.
    Kilowatts(f64) = Unit::Kilowatts, derives = []
}

unit_newtype! {
    /// A temperature in degrees Celsius.
    Celsius(f64) = Unit::Celsius, derives = []
}

unit_newtype! {
    /// A duration in whole minutes.
    Minutes(u32) = Unit::Minutes, derives = [Eq, Ord, Hash]
}

unit_newtype! {
    /// A number of days; negative when the date lies in the past.
    Days(i32) = Unit::Days, derives = [Eq, Ord, Hash]
}

/// A percentage from 0 to 100 inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct Percent(u8);

impl Percent {
    /// Zero percent.
    pub const ZERO: Percent = Percent(0);
    /// One hundred percent.
    pub const FULL: Percent = Percent(100);

    /// Wraps a value, rejecting anything above 100.
    pub const fn try_new(value: u8) -> Result<Self, DomainError> {
        if value > 100 {
            Err(DomainError::PercentOutOfRange { value })
        } else {
            Ok(Self(value))
        }
    }

    /// The raw value.
    pub const fn value(self) -> u8 {
        self.0
    }
}

impl HasUnit for Percent {
    const UNIT: Unit = Unit::Percent;
}

impl TryFrom<u8> for Percent {
    type Error = DomainError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl<'de> Deserialize<'de> for Percent {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u8::deserialize(deserializer)?;
        Percent::try_new(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_rejects_values_above_100() {
        assert_eq!(Percent::try_new(100), Ok(Percent::FULL));
        assert_eq!(
            Percent::try_new(101),
            Err(DomainError::PercentOutOfRange { value: 101 })
        );
        assert!(serde_json::from_str::<Percent>("101").is_err());
        assert_eq!(serde_json::from_str::<Percent>("42").unwrap().value(), 42);
    }

    #[test]
    fn newtypes_serialise_as_bare_numbers() {
        assert_eq!(
            serde_json::to_string(&Kilometres::new(12345)).unwrap(),
            "12345"
        );
        assert_eq!(
            serde_json::to_string(&ServiceKilometres::new(-120)).unwrap(),
            "-120"
        );
        assert_eq!(
            serde_json::to_string(&Kilowatts::new(11.0)).unwrap(),
            "11.0"
        );
        assert_eq!(serde_json::to_string(&Celsius::new(21.5)).unwrap(), "21.5");
        assert_eq!(serde_json::to_string(&Minutes::new(45)).unwrap(), "45");
        assert_eq!(serde_json::to_string(&Days::new(-3)).unwrap(), "-3");
        assert_eq!(serde_json::to_string(&Percent::FULL).unwrap(), "100");
    }

    #[test]
    fn units_are_known_at_the_type_level() {
        assert_eq!(Kilometres::UNIT, Unit::Kilometres);
        assert_eq!(ServiceKilometres::UNIT, Unit::Kilometres);
        assert_eq!(Percent::UNIT, Unit::Percent);
        assert_eq!(Kilowatts::UNIT, Unit::Kilowatts);
        assert_eq!(Celsius::UNIT, Unit::Celsius);
        assert_eq!(Minutes::UNIT, Unit::Minutes);
        assert_eq!(Days::UNIT, Unit::Days);
        assert_eq!(<bool as HasUnit>::UNIT, Unit::None);
        assert_eq!(Unit::Celsius.symbol(), "°C");
    }
}
