//! The field catalogue of the **continuous** packages, the ones the portal produces every fifteen
//! minutes.
//!
//! It differs from the catalogue of the one-time export (see [`super::normalise`]): flat snake case
//! names such as `state_of_charge` and `mileage`, integer codes instead of words. Names, units and
//! codes follow the portal's official data dictionary, "DataDictionary V5.0, Continuous Data",
//! which evcc keeps as JSON:
//! <https://github.com/evcc-io/evcc/blob/master/vehicle/vw/eudataact/datadictionary.json>.
//!
//! The codes the dictionary uses for states are `0` unsupported, `1` invalid, and then two values
//! with a documented meaning, for example `2` open and `3` closed. `0` leaves a value unsupported,
//! `1` makes it unavailable, and anything undocumented is unavailable too: nothing is guessed.
//! The meaning of `2` and `3` for doors and locks was checked against the real car on 2026-10-10.
//!
//! A value the car delivers as an **empty string** is unsupported, not unavailable: an electric
//! car sends the oil change interval empty in every package, because it has none. A field that is
//! missing from the package altogether is unavailable.

use std::collections::BTreeMap;

use vag2mqtt_domain::state::{DoorPosition, LockState, OpenState, WindowPosition};
use vag2mqtt_domain::units::{Celsius, Days, Kilometres, Percent, ServiceKilometres};
use vag2mqtt_domain::{Reading, VehicleState};

use super::document::{Point, Points};

/// State of charge in percent.
const SOC: &str = "state_of_charge";
/// Combined range of both engines in km; for an electric car the only one.
const RANGE: &str = "cruising_range_combined";
/// Range of the primary engine in km, used when the combined one is missing.
const RANGE_PRIMARY: &str = "cruising_range_primary_engine";
/// Odometer in km.
const MILEAGE: &str = "mileage";
/// Outside temperature in deci-Kelvin.
const OUTSIDE_TEMPERATURE: &str = "outside_temperature";
/// Days until the inspection; negative once it is overdue.
const INSPECTION_DAYS: &str = "maintenance_interval__time_until_inspection";
/// Kilometres until the inspection; negative once it is overdue.
const INSPECTION_KM: &str = "maintenance_interval_distance_until_inspection";
/// Days until the oil change; negative once it is overdue.
const OIL_DAYS: &str = "maintenance_interval__time_until_oil_change";
/// Kilometres until the oil change; negative once it is overdue.
const OIL_KM: &str = "maintenance_interval_distance_until_oil_change";

/// Door open states: `2` open, `3` closed.
const DOOR_OPEN: &[(DoorPosition, &str)] = &[
    (DoorPosition::FrontLeft, "open_state_front_left_door"),
    (DoorPosition::FrontRight, "open_state_front_right_door"),
    (DoorPosition::RearLeft, "open_state_rear_left_door"),
    (DoorPosition::RearRight, "open_state_rear_right_door"),
    (DoorPosition::Trunk, "open_state_tailgate"),
    (DoorPosition::Bonnet, "open_state_front_engine_bonnet"),
];

/// Lock states: `2` locked, `3` unlocked. The packages spell the rear left door with a double
/// underscore; both spellings are read.
const DOOR_LOCKS: &[&str] = &[
    "locked_state_front_left_door",
    "locked_state_front_right_door",
    "locked_state_rear_left_door",
    "locked_state__rear_left_door",
    "locked_state_rear_right_door",
    "locked_state_tailgate",
];

/// Window states: `2` open, `3` closed. The sunroof is reported as "sunroof motor hood 1"; a car
/// without one sends `0`, which leaves it unsupported.
const WINDOWS: &[(WindowPosition, &str)] = &[
    (WindowPosition::SunRoof, "state_sunroof_motor_hood_1"),
    (
        WindowPosition::FrontLeft,
        "state_front_left_door_window_lifter",
    ),
    (
        WindowPosition::FrontRight,
        "state_front_right_door_window_lifter",
    ),
    (
        WindowPosition::RearLeft,
        "state_rear_left_door_window_lifter",
    ),
    (
        WindowPosition::RearRight,
        "state_rear_right_door_window_lifter",
    ),
];

/// Every field of this catalogue the normaliser reads.
pub(crate) fn fields() -> impl Iterator<Item = &'static str> {
    [
        SOC,
        RANGE,
        RANGE_PRIMARY,
        MILEAGE,
        OUTSIDE_TEMPERATURE,
        INSPECTION_DAYS,
        INSPECTION_KM,
        OIL_DAYS,
        OIL_KM,
    ]
    .into_iter()
    .chain(DOOR_OPEN.iter().map(|(_, name)| *name))
    .chain(DOOR_LOCKS.iter().copied())
    .chain(WINDOWS.iter().map(|(_, name)| *name))
}

/// Fills `state` from the continuous catalogue, if any of its fields has arrived.
///
/// Fields both catalogues know (SoC, range, odometer) keep whichever value the vehicle captured
/// later. Categories only this catalogue knows (doors, windows, outside temperature, service) stay
/// unsupported until the first continuous package arrives, and are unavailable afterwards when a
/// field is missing.
pub(crate) fn apply(points: &Points, state: &mut VehicleState) {
    if !fields().any(|name| points.get(name).is_some()) {
        return;
    }

    let soc = reading(points, SOC, |v| {
        let n = int(v)?;
        u8::try_from(n).ok().and_then(|p| Percent::try_new(p).ok())
    });
    state.battery.soc = newer(
        std::mem::replace(&mut state.battery.soc, Reading::Unavailable),
        soc,
    );

    let km = |v: &str| u32::try_from(int(v)?).ok().map(Kilometres::new);
    let range = match reading(points, RANGE, km) {
        Reading::Present(sample) => Reading::Present(sample),
        _ => reading(points, RANGE_PRIMARY, km),
    };
    state.battery.range = newer(
        std::mem::replace(&mut state.battery.range, Reading::Unavailable),
        range,
    );
    let odometer = reading(points, MILEAGE, km);
    state.odometer = newer(
        std::mem::replace(&mut state.odometer, Reading::Unavailable),
        odometer,
    );

    state.status.outside_temperature = reading(points, OUTSIDE_TEMPERATURE, |v| {
        // 273.15 K is 2731.5 dK; subtracting in deci-Kelvin first keeps 2846 dK at 11.45 °C.
        let celsius = (number(v)? - 2731.5) / 10.0;
        (-60.0..=70.0)
            .contains(&celsius)
            .then(|| Celsius::new(celsius))
    });

    state.service.inspection_due_in = reading(points, INSPECTION_DAYS, |v| int32(v).map(Days::new));
    state.service.inspection_due_after = reading(points, INSPECTION_KM, |v| {
        int32(v).map(ServiceKilometres::new)
    });
    state.service.oil_service_due_in = reading(points, OIL_DAYS, |v| int32(v).map(Days::new));
    state.service.oil_service_due_after =
        reading(points, OIL_KM, |v| int32(v).map(ServiceKilometres::new));

    // Doors: one reading per position the car reports, and the aggregate.
    let mut doors = BTreeMap::new();
    for (position, name) in DOOR_OPEN {
        if let Some(reading) = coded(points, name, open_state) {
            doors.insert(*position, reading);
        }
    }
    state.doors.overall = aggregate_open(doors.values());
    state.doors.by_position = doors;

    let locks: Vec<Reading<LockState>> = DOOR_LOCKS
        .iter()
        .filter_map(|name| coded(points, name, lock_state))
        .collect();
    state.doors.lock = aggregate_lock(&locks);

    let mut windows = BTreeMap::new();
    for (position, name) in WINDOWS {
        if let Some(reading) = coded(points, name, open_state) {
            windows.insert(*position, reading);
        }
    }
    state.windows.overall = aggregate_open(windows.values());
    state.windows.by_position = windows;
}

/// A plain value: present when it arrived and parses, unavailable otherwise.
/// Unsupported when the car sends the field empty.
fn reading<T>(points: &Points, name: &str, parse: impl FnOnce(&str) -> Option<T>) -> Reading<T> {
    match points.get(name) {
        Some(point) if point.value.trim().is_empty() => Reading::Unsupported,
        Some(point) => match parse(&point.value) {
            Some(value) => Reading::present(value, point.source_time),
            None => Reading::Unavailable,
        },
        None => Reading::Unavailable,
    }
}

/// A coded state: `None` when the car does not report the field or reports it unsupported (`0`),
/// unavailable for invalid (`1`) or an undocumented code, present otherwise.
fn coded<T>(points: &Points, name: &str, decode: fn(i64) -> Option<T>) -> Option<Reading<T>> {
    let point: &Point = points.get(name)?;
    if point.value.trim().is_empty() {
        return None;
    }
    match int(&point.value) {
        Some(0) => None,
        Some(code) => Some(match decode(code) {
            Some(value) => Reading::present(value, point.source_time),
            None => Reading::Unavailable,
        }),
        None => Some(Reading::Unavailable),
    }
}

fn open_state(code: i64) -> Option<OpenState> {
    match code {
        2 => Some(OpenState::Open),
        3 => Some(OpenState::Closed),
        _ => None,
    }
}

fn lock_state(code: i64) -> Option<LockState> {
    match code {
        2 => Some(LockState::Locked),
        3 => Some(LockState::Unlocked),
        _ => None,
    }
}

/// Open if any part is open, closed if every reported part is closed, unavailable otherwise. The
/// time is the latest of the parts.
fn aggregate_open<'a>(parts: impl Iterator<Item = &'a Reading<OpenState>>) -> Reading<OpenState> {
    let parts: Vec<&Reading<OpenState>> = parts.collect();
    if parts.is_empty() {
        return Reading::Unavailable;
    }
    let time = parts
        .iter()
        .filter_map(|r| r.as_present().and_then(|s| s.source_time))
        .max();
    if parts.iter().any(|r| r.value() == Some(&OpenState::Open)) {
        Reading::present(OpenState::Open, time)
    } else if parts.iter().all(|r| r.value() == Some(&OpenState::Closed)) {
        Reading::present(OpenState::Closed, time)
    } else {
        Reading::Unavailable
    }
}

/// Unlocked if any door is unlocked, locked if every reported door is locked.
fn aggregate_lock(parts: &[Reading<LockState>]) -> Reading<LockState> {
    if parts.is_empty() {
        return Reading::Unavailable;
    }
    let time = parts
        .iter()
        .filter_map(|r| r.as_present().and_then(|s| s.source_time))
        .max();
    if parts
        .iter()
        .any(|r| r.value() == Some(&LockState::Unlocked))
    {
        Reading::present(LockState::Unlocked, time)
    } else if parts.iter().all(|r| r.value() == Some(&LockState::Locked)) {
        Reading::present(LockState::Locked, time)
    } else {
        Reading::Unavailable
    }
}

/// Of two readings of the same value from different catalogues, the one captured later. A present
/// value always beats a missing one.
fn newer<T>(current: Reading<T>, candidate: Reading<T>) -> Reading<T> {
    match (&current, &candidate) {
        (Reading::Present(old), Reading::Present(new)) if old.source_time > new.source_time => {
            current
        }
        (_, Reading::Present(_)) => candidate,
        _ => current,
    }
}

fn number(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn int(value: &str) -> Option<i64> {
    let n = number(value)?;
    (n.fract() == 0.0 && n.abs() < 1e12).then_some(n as i64)
}

fn int32(value: &str) -> Option<i32> {
    int(value).and_then(|n| i32::try_from(n).ok())
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::export::document::read_package;
    use crate::export::normalise::{is_relevant, normalise};

    fn fixture(name: &str) -> Points {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/audi/eudataact")
                .join(name),
        )
        .unwrap();
        let mut points = Points::default();
        points.merge(read_package(&bytes).unwrap(), is_relevant);
        points
    }

    fn value<T: Clone>(reading: &Reading<T>) -> T {
        reading.value().cloned().expect("a present value")
    }

    #[test]
    fn the_continuous_package_fills_the_model() {
        let state = normalise(&fixture("continuous_package.json"), Utc::now());
        assert_eq!(value(&state.battery.soc), Percent::try_new(35).unwrap());
        assert_eq!(value(&state.battery.range), Kilometres::new(125));
        assert_eq!(value(&state.odometer), Kilometres::new(1217));
        let outside = value(&state.status.outside_temperature).value();
        assert!((outside - 11.45).abs() < 1e-9, "{outside}");
        assert_eq!(value(&state.service.inspection_due_in), Days::new(-699));
        // Sent empty in every package: this electric car has no such interval.
        assert!(!state.service.inspection_due_after.is_supported());
        assert!(!state.service.oil_service_due_in.is_supported());
        assert!(!state.service.oil_service_due_after.is_supported());
    }

    #[test]
    fn the_sunroof_is_read_and_left_out_when_the_car_has_none() {
        // The real car sends `0`: no sunroof.
        let state = normalise(&fixture("continuous_package.json"), Utc::now());
        assert!(
            !state
                .windows
                .by_position
                .contains_key(&WindowPosition::SunRoof)
        );
        // A car that has one reports it like any window.
        let mut points = Points::default();
        points.merge(
            read_package(
                br#"{"Data":[{"dataFieldName":"state_sunroof_motor_hood_1","value":"2"}]}"#,
            )
            .unwrap(),
            is_relevant,
        );
        let state = normalise(&points, Utc::now());
        assert_eq!(
            value(&state.windows.by_position[&WindowPosition::SunRoof]),
            OpenState::Open
        );
        assert_eq!(value(&state.windows.overall), OpenState::Open);
    }

    #[test]
    fn empty_means_unsupported_and_missing_means_unavailable() {
        let mut points = Points::default();
        points.merge(
            read_package(
                br#"{"Data":[
                    {"dataFieldName":"maintenance_interval__time_until_oil_change","value":""},
                    {"dataFieldName":"mileage","value":"1233"}
                ]}"#,
            )
            .unwrap(),
            is_relevant,
        );
        let state = normalise(&points, Utc::now());
        assert!(!state.service.oil_service_due_in.is_supported());
        assert!(matches!(
            state.service.inspection_due_in,
            Reading::Unavailable
        ));
    }

    #[test]
    fn doors_locks_and_windows_follow_the_dictionary() {
        let state = normalise(&fixture("continuous_package.json"), Utc::now());
        // As the car really was: unlocked, driver's door open, everything else closed.
        assert_eq!(value(&state.doors.lock), LockState::Unlocked);
        assert_eq!(value(&state.doors.overall), OpenState::Open);
        assert_eq!(
            value(&state.doors.by_position[&DoorPosition::FrontLeft]),
            OpenState::Open
        );
        for position in [
            DoorPosition::FrontRight,
            DoorPosition::RearLeft,
            DoorPosition::RearRight,
            DoorPosition::Trunk,
            DoorPosition::Bonnet,
        ] {
            assert_eq!(
                value(&state.doors.by_position[&position]),
                OpenState::Closed
            );
        }
        assert_eq!(value(&state.windows.overall), OpenState::Closed);
        assert_eq!(state.windows.by_position.len(), 4);
    }

    #[test]
    fn what_the_continuous_packages_lack_stays_honest() {
        let state = normalise(&fixture("continuous_package.json"), Utc::now());
        assert!(!state.position.is_supported(), "no GPS in the export");
        assert!(matches!(state.charging.state, Reading::Unavailable));
        assert!(matches!(state.plug.connection, Reading::Unavailable));
        assert!(matches!(state.climatisation.state, Reading::Unavailable));
    }

    #[test]
    fn without_a_continuous_package_its_categories_stay_unsupported() {
        let state = normalise(&fixture("one_time_export.json"), Utc::now());
        assert!(!state.doors.lock.is_supported());
        assert!(!state.windows.overall.is_supported());
        assert!(!state.status.outside_temperature.is_supported());
        assert_eq!(value(&state.battery.soc), Percent::try_new(51).unwrap());
    }

    #[test]
    fn the_newer_capture_wins_across_catalogues() {
        // The one-time export reports 51 % on 2026-09-23, the continuous package 35 % on
        // 2026-10-09: the later one wins, whichever catalogue it came from.
        let mut points = fixture("one_time_export.json");
        let continuous = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/audi/eudataact/continuous_package.json"),
        )
        .unwrap();
        points.merge(read_package(&continuous).unwrap(), is_relevant);
        let state = normalise(&points, Utc::now());
        assert_eq!(value(&state.battery.soc), Percent::try_new(35).unwrap());
        assert_eq!(value(&state.odometer), Kilometres::new(1217));
        // A field only the one-time export carries is still there.
        assert_eq!(
            value(&state.charging.target_soc),
            Percent::try_new(80).unwrap()
        );
    }

    #[test]
    fn codes_outside_the_dictionary_are_not_guessed() {
        let mut points = Points::default();
        points.merge(
            read_package(
                br#"{"Data":[
                    {"dataFieldName":"open_state_front_left_door","value":"0"},
                    {"dataFieldName":"open_state_front_right_door","value":"1"},
                    {"dataFieldName":"open_state_rear_left_door","value":"7"},
                    {"dataFieldName":"open_state_rear_right_door","value":"3"},
                    {"dataFieldName":"locked_state_front_left_door","value":"2"},
                    {"dataFieldName":"outside_temperature","value":"99999"}
                ]}"#,
            )
            .unwrap(),
            is_relevant,
        );
        let state = normalise(&points, Utc::now());
        let doors = &state.doors.by_position;
        assert!(
            !doors.contains_key(&DoorPosition::FrontLeft),
            "0 is unsupported"
        );
        assert!(matches!(
            doors[&DoorPosition::FrontRight],
            Reading::Unavailable
        ));
        assert!(matches!(
            doors[&DoorPosition::RearLeft],
            Reading::Unavailable
        ));
        assert_eq!(value(&doors[&DoorPosition::RearRight]), OpenState::Closed);
        assert!(
            matches!(state.doors.overall, Reading::Unavailable),
            "not every door is known closed"
        );
        assert_eq!(value(&state.doors.lock), LockState::Locked);
        assert!(matches!(
            state.status.outside_temperature,
            Reading::Unavailable
        ));
    }
}
