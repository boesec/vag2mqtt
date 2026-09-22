//! Vehicle position.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::DomainError;
use crate::units::{HasUnit, Unit};

/// A geographic position. One value, because latitude and longitude are meaningless apart.
///
/// `Debug` is redacted: coordinates are never logged (NFR-007). `Display` is not implemented at
/// all, so the only way to get the numbers out is through the accessors.
#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawGeoPosition")]
pub struct GeoPosition {
    latitude: f64,
    longitude: f64,
    heading_degrees: Option<f64>,
}

impl GeoPosition {
    /// Builds a position, rejecting a latitude outside ±90, a longitude outside ±180 or a
    /// heading outside 0 to 360.
    pub fn try_new(
        latitude: f64,
        longitude: f64,
        heading_degrees: Option<f64>,
    ) -> Result<Self, DomainError> {
        if !(-90.0..=90.0).contains(&latitude) {
            return Err(DomainError::CoordinateOutOfRange { which: "latitude" });
        }
        if !(-180.0..=180.0).contains(&longitude) {
            return Err(DomainError::CoordinateOutOfRange { which: "longitude" });
        }
        if let Some(heading) = heading_degrees
            && !(0.0..=360.0).contains(&heading)
        {
            return Err(DomainError::CoordinateOutOfRange { which: "heading" });
        }
        Ok(Self {
            latitude,
            longitude,
            heading_degrees,
        })
    }

    /// Latitude in degrees, north positive.
    pub fn latitude(&self) -> f64 {
        self.latitude
    }

    /// Longitude in degrees, east positive.
    pub fn longitude(&self) -> f64 {
        self.longitude
    }

    /// Heading in degrees clockwise from north, if the manufacturer delivers one.
    pub fn heading_degrees(&self) -> Option<f64> {
        self.heading_degrees
    }
}

impl fmt::Debug for GeoPosition {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GeoPosition(redacted)")
    }
}

impl HasUnit for GeoPosition {
    const UNIT: Unit = Unit::Degrees;
}

/// The unvalidated wire form; deserialisation goes through [`GeoPosition::try_new`].
#[derive(Deserialize)]
struct RawGeoPosition {
    latitude: f64,
    longitude: f64,
    #[serde(default)]
    heading_degrees: Option<f64>,
}

impl TryFrom<RawGeoPosition> for GeoPosition {
    type Error = DomainError;

    fn try_from(raw: RawGeoPosition) -> Result<Self, Self::Error> {
        GeoPosition::try_new(raw.latitude, raw.longitude, raw.heading_degrees)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_contains_no_digit() {
        let position = GeoPosition::try_new(48.1, 11.5, Some(270.0)).unwrap();
        let debug = format!("{position:?}");
        assert!(!debug.chars().any(|c| c.is_ascii_digit()), "{debug}");
    }

    #[test]
    fn out_of_range_coordinates_are_rejected() {
        assert_eq!(
            GeoPosition::try_new(90.5, 0.0, None),
            Err(DomainError::CoordinateOutOfRange { which: "latitude" })
        );
        assert_eq!(
            GeoPosition::try_new(0.0, -180.5, None),
            Err(DomainError::CoordinateOutOfRange { which: "longitude" })
        );
        assert_eq!(
            GeoPosition::try_new(0.0, 0.0, Some(361.0)),
            Err(DomainError::CoordinateOutOfRange { which: "heading" })
        );
        assert!(
            serde_json::from_str::<GeoPosition>(r#"{"latitude":91.0,"longitude":0.0}"#).is_err()
        );
    }

    #[test]
    fn serde_round_trips_with_and_without_heading() {
        let with = GeoPosition::try_new(48.1, 11.5, Some(270.0)).unwrap();
        let json = serde_json::to_string(&with).unwrap();
        assert_eq!(
            json,
            r#"{"latitude":48.1,"longitude":11.5,"heading_degrees":270.0}"#
        );
        assert_eq!(serde_json::from_str::<GeoPosition>(&json).unwrap(), with);

        let without: GeoPosition =
            serde_json::from_str(r#"{"latitude":48.1,"longitude":11.5}"#).unwrap();
        assert_eq!(without.heading_degrees(), None);
    }
}
