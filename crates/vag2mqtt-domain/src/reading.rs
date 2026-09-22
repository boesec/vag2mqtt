//! The three state value container from requirements section 4.3.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One vehicle value in exactly one of three states.
///
/// There is deliberately no `Default`: a connector must decide for every field which state
/// applies (ER-007). Missing fields in a manufacturer response become [`Reading::Unavailable`]
/// when the capability is known to exist, and [`Reading::Unsupported`] when the vehicle declares
/// that it lacks the capability.
///
/// Serde form, tagged on `state`:
///
/// ```json
/// {"state": "unsupported"}
/// {"state": "unavailable"}
/// {"state": "present", "value": 12345, "source_time": "2026-09-22T10:00:00Z"}
/// ```
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Reading<T> {
    /// The vehicle does not have this capability. Never published on MQTT.
    Unsupported,
    /// The capability exists but the last fetch delivered no value. The last retained value
    /// stays on MQTT; availability reflects the gap.
    Unavailable,
    /// A value with its manufacturer timestamp.
    Present(Sample<T>),
}

/// A present value together with the manufacturer's own timestamp for it.
///
/// The fetch time is not here: it is recorded once per snapshot on
/// [`VehicleState::fetched_at`](crate::VehicleState::fetched_at).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample<T> {
    /// The value.
    pub value: T,
    /// When the manufacturer says the value was captured, if it says so at all.
    pub source_time: Option<DateTime<Utc>>,
}

impl<T> Reading<T> {
    /// A present value with an optional manufacturer timestamp.
    pub fn present(value: T, source_time: Option<DateTime<Utc>>) -> Self {
        Reading::Present(Sample { value, source_time })
    }

    /// `true` unless the vehicle lacks the capability.
    pub fn is_supported(&self) -> bool {
        !matches!(self, Reading::Unsupported)
    }

    /// `true` if a value is present.
    pub fn is_present(&self) -> bool {
        matches!(self, Reading::Present(_))
    }

    /// The sample, if a value is present.
    pub fn as_present(&self) -> Option<&Sample<T>> {
        match self {
            Reading::Present(sample) => Some(sample),
            _ => None,
        }
    }

    /// The value, if present.
    pub fn value(&self) -> Option<&T> {
        self.as_present().map(|sample| &sample.value)
    }

    /// Applies `f` to a present value and keeps the other two states as they are.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Reading<U> {
        match self {
            Reading::Unsupported => Reading::Unsupported,
            Reading::Unavailable => Reading::Unavailable,
            Reading::Present(Sample { value, source_time }) => Reading::Present(Sample {
                value: f(value),
                source_time,
            }),
        }
    }

    /// Borrows the value without consuming the reading.
    pub fn by_ref(&self) -> Reading<&T> {
        match self {
            Reading::Unsupported => Reading::Unsupported,
            Reading::Unavailable => Reading::Unavailable,
            Reading::Present(Sample { value, source_time }) => Reading::Present(Sample {
                value,
                source_time: *source_time,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 22, hour, 0, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn serde_forms_are_tagged_on_state() {
        let unsupported: Reading<u32> = Reading::Unsupported;
        let unavailable: Reading<u32> = Reading::Unavailable;
        let present = Reading::present(12345u32, Some(at(10)));
        let untimed = Reading::present("charging", None);

        assert_eq!(
            serde_json::to_string(&unsupported).unwrap(),
            r#"{"state":"unsupported"}"#
        );
        assert_eq!(
            serde_json::to_string(&unavailable).unwrap(),
            r#"{"state":"unavailable"}"#
        );
        assert_eq!(
            serde_json::to_string(&present).unwrap(),
            r#"{"state":"present","value":12345,"source_time":"2026-09-22T10:00:00Z"}"#
        );
        assert_eq!(
            serde_json::to_string(&untimed).unwrap(),
            r#"{"state":"present","value":"charging","source_time":null}"#
        );
    }

    #[test]
    fn serde_round_trips_every_state() {
        for reading in [
            Reading::Unsupported,
            Reading::Unavailable,
            Reading::present(7u8, None),
            Reading::present(9u8, Some(at(11))),
        ] {
            let json = serde_json::to_string(&reading).unwrap();
            let back: Reading<u8> = serde_json::from_str(&json).unwrap();
            assert_eq!(back, reading);
        }
    }

    #[test]
    fn map_keeps_the_state_and_the_timestamp() {
        let present = Reading::present(2u32, Some(at(12))).map(|v| v * 10);
        assert_eq!(present, Reading::present(20u32, Some(at(12))));
        let unavailable: Reading<u32> = Reading::Unavailable;
        assert_eq!(unavailable.map(|v| v * 10), Reading::Unavailable);
        let unsupported: Reading<u32> = Reading::Unsupported;
        assert_eq!(unsupported.map(|v| v * 10), Reading::Unsupported);
    }

    #[test]
    fn accessors_reflect_the_state() {
        let present = Reading::present(1u8, None);
        assert!(present.is_supported());
        assert!(present.is_present());
        assert_eq!(present.value(), Some(&1));
        assert_eq!(present.by_ref().value(), Some(&&1));

        let unavailable: Reading<u8> = Reading::Unavailable;
        assert!(unavailable.is_supported());
        assert!(!unavailable.is_present());
        assert_eq!(unavailable.value(), None);

        let unsupported: Reading<u8> = Reading::Unsupported;
        assert!(!unsupported.is_supported());
        assert_eq!(unsupported.as_present(), None);
    }
}
