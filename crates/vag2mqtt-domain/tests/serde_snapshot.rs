//! Pins the serde representation of a fully populated `VehicleState`.
//!
//! The fixture under `tests/fixtures/vehicle_state_full.json` is the contract other crates and
//! the MQTT `full` payload (WP-04) build on. Changing it is a deliberate act:
//!
//! ```text
//! cargo test -p vag2mqtt-domain --test serde_snapshot -- --ignored write_snapshot
//! ```
//!
//! rewrites the file from the builder below; review the diff before committing it.

// Integration tests may panic on malformed fixtures; helper functions here are test code too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, TimeZone, Utc};
use vag2mqtt_domain::state::{
    Battery, ChargeCurrentType, Charging, ChargingMode, ChargingState, Climatisation,
    ClimatisationState, DoorPosition, Doors, ExternalPower, Fuel, GeoPosition, LockState,
    OpenState, Plug, PlugConnection, Service, VehicleActivity, VehicleConnection, VehicleStatus,
    WindowPosition, Windows,
};
use vag2mqtt_domain::units::{
    Celsius, Days, Kilometres, Kilowatts, Minutes, Percent, ServiceKilometres,
};
use vag2mqtt_domain::{Reading, VehicleState};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("vehicle_state_full.json")
}

fn at(minute: u32) -> Option<DateTime<Utc>> {
    Utc.with_ymd_and_hms(2026, 9, 22, 10, minute, 0).single()
}

fn percent(value: u8) -> Percent {
    Percent::try_new(value).expect("fixture percentages are valid")
}

/// A synthetic snapshot in which every category carries a value, and one field per category
/// exercises `Unavailable` or `Unsupported`, so the fixture shows all three states.
fn full_state() -> VehicleState {
    let fetched_at = Utc
        .with_ymd_and_hms(2026, 9, 22, 10, 30, 0)
        .single()
        .expect("valid timestamp");

    let mut doors_by_position = BTreeMap::new();
    for position in DoorPosition::ALL {
        doors_by_position.insert(*position, Reading::present(OpenState::Closed, at(5)));
    }
    doors_by_position.insert(DoorPosition::Bonnet, Reading::Unsupported);

    let mut windows_by_position = BTreeMap::new();
    for position in WindowPosition::ALL {
        windows_by_position.insert(*position, Reading::present(OpenState::Closed, at(5)));
    }
    windows_by_position.insert(WindowPosition::SunRoof, Reading::Unavailable);

    VehicleState {
        fetched_at,
        odometer: Reading::present(Kilometres::new(12_345), at(1)),
        battery: Battery {
            soc: Reading::present(percent(80), at(2)),
            range: Reading::present(Kilometres::new(410), at(2)),
        },
        fuel: Fuel {
            level: Reading::Unsupported,
            range: Reading::Unsupported,
        },
        charging: Charging {
            state: Reading::present(ChargingState::Charging, at(3)),
            mode: Reading::present(ChargingMode::Manual, at(3)),
            current_type: Reading::present(ChargeCurrentType::Ac, at(3)),
            power: Reading::present(Kilowatts::new(11.0), at(3)),
            remaining_time: Reading::present(Minutes::new(95), at(3)),
            target_soc: Reading::present(percent(90), None),
        },
        plug: Plug {
            connection: Reading::present(PlugConnection::Connected, at(3)),
            lock: Reading::present(LockState::Locked, at(3)),
            external_power: Reading::present(ExternalPower::Active, at(3)),
        },
        doors: Doors {
            overall: Reading::present(OpenState::Closed, at(5)),
            lock: Reading::present(LockState::Locked, at(5)),
            by_position: doors_by_position,
        },
        windows: Windows {
            overall: Reading::present(OpenState::Closed, at(5)),
            by_position: windows_by_position,
        },
        position: Reading::present(
            GeoPosition::try_new(51.0, 9.0, Some(180.0)).expect("valid fixture position"),
            at(6),
        ),
        climatisation: Climatisation {
            state: Reading::present(ClimatisationState::Heating, at(7)),
            target_temperature: Reading::present(Celsius::new(21.5), at(7)),
            remaining_time: Reading::Unavailable,
        },
        status: VehicleStatus {
            connection: Reading::present(VehicleConnection::Online, at(8)),
            activity: Reading::present(VehicleActivity::Parked, at(8)),
            secured: Reading::present(true, at(8)),
            outside_temperature: Reading::present(Celsius::new(14.0), at(8)),
        },
        service: Service {
            inspection_due_in: Reading::present(Days::new(120), at(9)),
            inspection_due_after: Reading::present(ServiceKilometres::new(-150), at(9)),
            oil_service_due_in: Reading::Unsupported,
            oil_service_due_after: Reading::Unsupported,
        },
    }
}

#[test]
fn full_state_matches_the_pinned_snapshot() {
    let expected_text = std::fs::read_to_string(fixture_path()).expect("fixture file present");
    let expected: serde_json::Value =
        serde_json::from_str(&expected_text).expect("fixture is valid JSON");
    let actual = serde_json::to_value(full_state()).expect("state serialises");

    if expected != actual {
        let actual_pretty = serde_json::to_string_pretty(&actual).expect("pretty print");
        panic!(
            "serde representation of VehicleState changed.\n\
             If this is intended, regenerate the fixture with\n\
             cargo test -p vag2mqtt-domain --test serde_snapshot -- --ignored write_snapshot\n\
             and review the diff.\n\nactual:\n{actual_pretty}"
        );
    }
}

#[test]
fn pinned_snapshot_round_trips() {
    let text = std::fs::read_to_string(fixture_path()).expect("fixture file present");
    let state: VehicleState = serde_json::from_str(&text).expect("fixture deserialises");
    assert_eq!(state, full_state());
}

#[test]
fn unsupported_state_has_the_same_shape_as_the_full_one() {
    let fetched_at = full_state().fetched_at;
    let unsupported =
        serde_json::to_value(VehicleState::unsupported(fetched_at)).expect("serialises");
    let full = serde_json::to_value(full_state()).expect("serialises");
    let unsupported_keys: Vec<_> = unsupported.as_object().expect("object").keys().collect();
    let full_keys: Vec<_> = full.as_object().expect("object").keys().collect();
    assert_eq!(unsupported_keys, full_keys);
}

#[test]
#[ignore = "regenerates the fixture; run on purpose"]
fn write_snapshot() {
    let pretty = serde_json::to_string_pretty(&full_state()).expect("state serialises");
    std::fs::write(fixture_path(), format!("{pretty}\n")).expect("fixture written");
}
