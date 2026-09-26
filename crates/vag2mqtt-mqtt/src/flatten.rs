//! Turns a `VehicleState` into the scalar topics of contract version 1.
//!
//! The path table here **is** the contract (`CONTRACT.md` in this crate); a test keeps the two in
//! sync. `Unsupported` and `Unavailable` readings produce no entry.

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use vag2mqtt_domain::state::{DoorPosition, WindowPosition};
use vag2mqtt_domain::{Reading, VehicleState};

/// One scalar value of the contract, relative to `status/<VIN>/`.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatValue {
    /// The path under `status/<VIN>/`, for example `battery/soc`.
    pub path: String,
    /// The `val` scalar.
    pub val: Value,
    /// The manufacturer's timestamp, or the snapshot's `fetched_at` when it has none.
    pub ts: DateTime<Utc>,
}

/// The position segment of a door topic, as in `doors/<position>/open`.
pub fn door_segment(position: DoorPosition) -> &'static str {
    match position {
        DoorPosition::FrontLeft => "front_left",
        DoorPosition::FrontRight => "front_right",
        DoorPosition::RearLeft => "rear_left",
        DoorPosition::RearRight => "rear_right",
        DoorPosition::Trunk => "trunk",
        DoorPosition::Bonnet => "bonnet",
    }
}

/// The position segment of a window topic, as in `windows/<position>/open`.
pub fn window_segment(position: WindowPosition) -> &'static str {
    match position {
        WindowPosition::FrontLeft => "front_left",
        WindowPosition::FrontRight => "front_right",
        WindowPosition::RearLeft => "rear_left",
        WindowPosition::RearRight => "rear_right",
        WindowPosition::SunRoof => "sun_roof",
    }
}

/// Every present value of the snapshot as a contract path with its scalar and timestamp.
pub fn flatten(state: &VehicleState) -> Vec<FlatValue> {
    let mut out = Flattener {
        fetched_at: state.fetched_at,
        values: Vec::with_capacity(48),
    };

    out.push("odometer", &state.odometer);
    out.push("battery/soc", &state.battery.soc);
    out.push("battery/range", &state.battery.range);
    out.push("fuel/level", &state.fuel.level);
    out.push("fuel/range", &state.fuel.range);
    out.push("charging/state", &state.charging.state);
    out.push("charging/mode", &state.charging.mode);
    out.push("charging/current_type", &state.charging.current_type);
    out.push("charging/power", &state.charging.power);
    out.push("charging/remaining_time", &state.charging.remaining_time);
    out.push("charging/target_soc", &state.charging.target_soc);
    out.push("plug/connection", &state.plug.connection);
    out.push("plug/lock", &state.plug.lock);
    out.push("plug/external_power", &state.plug.external_power);
    out.push("doors/open", &state.doors.overall);
    out.push("doors/lock", &state.doors.lock);
    for (position, reading) in &state.doors.by_position {
        out.push_owned(door_path(door_segment(*position)), reading);
    }
    out.push("windows/open", &state.windows.overall);
    for (position, reading) in &state.windows.by_position {
        out.push_owned(window_path(window_segment(*position)), reading);
    }
    if let Reading::Present(sample) = &state.position {
        let ts = sample.source_time.unwrap_or(state.fetched_at);
        out.raw(
            "position/latitude",
            Value::from(sample.value.latitude()),
            ts,
        );
        out.raw(
            "position/longitude",
            Value::from(sample.value.longitude()),
            ts,
        );
        if let Some(heading) = sample.value.heading_degrees() {
            out.raw("position/heading", Value::from(heading), ts);
        }
    }
    out.push("climatisation/state", &state.climatisation.state);
    out.push(
        "climatisation/target_temperature",
        &state.climatisation.target_temperature,
    );
    out.push(
        "climatisation/remaining_time",
        &state.climatisation.remaining_time,
    );
    out.push("status/connection", &state.status.connection);
    out.push("status/activity", &state.status.activity);
    out.push("status/secured", &state.status.secured);
    out.push(
        "status/outside_temperature",
        &state.status.outside_temperature,
    );
    out.push(
        "service/inspection_due_days",
        &state.service.inspection_due_in,
    );
    out.push(
        "service/inspection_due_km",
        &state.service.inspection_due_after,
    );
    out.push(
        "service/oil_service_due_days",
        &state.service.oil_service_due_in,
    );
    out.push(
        "service/oil_service_due_km",
        &state.service.oil_service_due_after,
    );

    out.values
}

/// The contract paths with a `<position>` placeholder, for documentation and tests.
pub const PATH_PATTERNS: &[&str] = &[
    "odometer",
    "battery/soc",
    "battery/range",
    "fuel/level",
    "fuel/range",
    "charging/state",
    "charging/mode",
    "charging/current_type",
    "charging/power",
    "charging/remaining_time",
    "charging/target_soc",
    "plug/connection",
    "plug/lock",
    "plug/external_power",
    "doors/open",
    "doors/lock",
    "doors/<position>/open",
    "windows/open",
    "windows/<position>/open",
    "position/latitude",
    "position/longitude",
    "position/heading",
    "climatisation/state",
    "climatisation/target_temperature",
    "climatisation/remaining_time",
    "status/connection",
    "status/activity",
    "status/secured",
    "status/outside_temperature",
    "service/inspection_due_days",
    "service/inspection_due_km",
    "service/oil_service_due_days",
    "service/oil_service_due_km",
];

/// Replaces a door or window position segment with `<position>` so a concrete path can be
/// matched against [`PATH_PATTERNS`].
pub fn pattern_of(path: &str) -> String {
    let mut segments: Vec<&str> = path.split('/').collect();
    if segments.len() == 3 && (segments[0] == "doors" || segments[0] == "windows") {
        segments[1] = "<position>";
    }
    segments.join("/")
}

fn door_path(segment: &str) -> String {
    let mut path = String::from("doors/");
    path.push_str(segment);
    path.push_str("/open");
    path
}

fn window_path(segment: &str) -> String {
    let mut path = String::from("windows/");
    path.push_str(segment);
    path.push_str("/open");
    path
}

struct Flattener {
    fetched_at: DateTime<Utc>,
    values: Vec<FlatValue>,
}

impl Flattener {
    fn push<T: Serialize>(&mut self, path: &str, reading: &Reading<T>) {
        self.push_owned(path.to_string(), reading);
    }

    fn push_owned<T: Serialize>(&mut self, path: String, reading: &Reading<T>) {
        if let Reading::Present(sample) = reading
            && let Ok(val) = serde_json::to_value(&sample.value)
        {
            self.values.push(FlatValue {
                path,
                val,
                ts: sample.source_time.unwrap_or(self.fetched_at),
            });
        }
    }

    fn raw(&mut self, path: &str, val: Value, ts: DateTime<Utc>) {
        self.values.push(FlatValue {
            path: path.to_string(),
            val,
            ts,
        });
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use vag2mqtt_domain::state::{GeoPosition, OpenState};
    use vag2mqtt_domain::units::{Kilometres, Percent};

    use super::*;

    #[test]
    fn unsupported_and_unavailable_produce_nothing() {
        let fetched_at = Utc
            .with_ymd_and_hms(2026, 9, 22, 10, 0, 0)
            .single()
            .unwrap();
        let mut state = VehicleState::unsupported(fetched_at);
        assert!(flatten(&state).is_empty());
        state.battery.soc = Reading::Unavailable;
        assert!(flatten(&state).is_empty());
    }

    #[test]
    fn present_values_get_their_path_scalar_and_timestamp() {
        let fetched_at = Utc
            .with_ymd_and_hms(2026, 9, 22, 10, 0, 0)
            .single()
            .unwrap();
        let source = Utc
            .with_ymd_and_hms(2026, 9, 22, 9, 55, 0)
            .single()
            .unwrap();
        let mut state = VehicleState::unsupported(fetched_at);
        state.odometer = Reading::present(Kilometres::new(12_345), Some(source));
        state.battery.soc = Reading::present(Percent::try_new(80).unwrap(), None);
        state.doors.overall = Reading::present(OpenState::Closed, None);
        state
            .doors
            .by_position
            .insert(DoorPosition::Trunk, Reading::present(OpenState::Ajar, None));
        state.position =
            Reading::present(GeoPosition::try_new(51.0, 9.0, None).unwrap(), Some(source));

        let flat = flatten(&state);
        let paths: Vec<&str> = flat.iter().map(|v| v.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "odometer",
                "battery/soc",
                "doors/open",
                "doors/trunk/open",
                "position/latitude",
                "position/longitude",
            ]
        );
        assert_eq!(flat[0].val, Value::from(12_345));
        assert_eq!(flat[0].ts, source);
        assert_eq!(flat[1].val, Value::from(80));
        assert_eq!(flat[1].ts, fetched_at);
        assert_eq!(flat[2].val, Value::from("closed"));
        assert_eq!(flat[3].val, Value::from("ajar"));
        assert_eq!(flat[4].val, Value::from(51.0));
        assert_eq!(flat[5].ts, source);
    }

    #[test]
    fn pattern_of_replaces_positions_only() {
        assert_eq!(pattern_of("doors/front_left/open"), "doors/<position>/open");
        assert_eq!(
            pattern_of("windows/sun_roof/open"),
            "windows/<position>/open"
        );
        assert_eq!(pattern_of("doors/open"), "doors/open");
        assert_eq!(pattern_of("battery/soc"), "battery/soc");
    }
}
