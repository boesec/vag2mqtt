//! Every categorical enum has a fixed set of wire names. Renaming a variant fails here first.

// Integration tests may panic on malformed fixtures; helper functions here are test code too.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use vag2mqtt_domain::state::{
    ChargeCurrentType, ChargingMode, ChargingState, ClimatisationState, DoorPosition,
    ExternalPower, LockState, OpenState, PlugConnection, VehicleActivity, VehicleConnection,
    WindowPosition,
};
use vag2mqtt_domain::{
    AccountConnectionState, Brand, DataSourceKind, Drivetrain, ErrorCategory, Unit,
    VehicleDataState,
};

fn assert_wire_names<T>(all: &[T], expected: &[&str])
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let actual: Vec<String> = all
        .iter()
        .map(|variant| {
            let json = serde_json::to_string(variant).expect("serialises");
            let back: T = serde_json::from_str(&json).expect("deserialises");
            assert_eq!(&back, variant);
            json.trim_matches('"').to_string()
        })
        .collect();
    assert_eq!(
        actual,
        expected,
        "wire names of {}",
        std::any::type_name::<T>()
    );
}

#[test]
fn charging_enums() {
    assert_wire_names(
        ChargingState::ALL,
        &[
            "off",
            "ready_for_charging",
            "charging",
            "conservation",
            "discharging",
            "error",
            "unknown",
        ],
    );
    assert_wire_names(
        ChargingMode::ALL,
        &["manual", "timer", "preferred_times", "off", "unknown"],
    );
    assert_wire_names(ChargeCurrentType::ALL, &["ac", "dc", "unknown"]);
}

#[test]
fn plug_enums() {
    assert_wire_names(
        PlugConnection::ALL,
        &["connected", "disconnected", "unknown"],
    );
    assert_wire_names(
        ExternalPower::ALL,
        &["available", "active", "unavailable", "unknown"],
    );
}

#[test]
fn access_enums() {
    assert_wire_names(OpenState::ALL, &["open", "closed", "ajar", "unknown"]);
    assert_wire_names(LockState::ALL, &["locked", "unlocked", "unknown"]);
    assert_wire_names(
        DoorPosition::ALL,
        &[
            "front_left",
            "front_right",
            "rear_left",
            "rear_right",
            "trunk",
            "bonnet",
        ],
    );
    assert_wire_names(
        WindowPosition::ALL,
        &[
            "front_left",
            "front_right",
            "rear_left",
            "rear_right",
            "sun_roof",
        ],
    );
}

#[test]
fn climatisation_and_status_enums() {
    assert_wire_names(
        ClimatisationState::ALL,
        &["off", "heating", "cooling", "ventilation", "unknown"],
    );
    assert_wire_names(VehicleConnection::ALL, &["online", "offline", "unknown"]);
    assert_wire_names(
        VehicleActivity::ALL,
        &["parked", "ignition_on", "driving", "unknown"],
    );
}

#[test]
fn account_and_vehicle_enums() {
    assert_wire_names(
        AccountConnectionState::ALL,
        &[
            "pending",
            "ok",
            "auth_error",
            "rate_limited",
            "unreachable",
            "error",
            "disabled",
        ],
    );
    assert_wire_names(
        VehicleDataState::ALL,
        &[
            "pending",
            "fresh",
            "stale",
            "unavailable",
            "error",
            "disabled",
        ],
    );
    assert_wire_names(
        Brand::ALL,
        &["audi", "volkswagen", "skoda", "seat", "cupra"],
    );
    assert_wire_names(DataSourceKind::ALL, &["live_api", "export"]);
    assert_wire_names(
        Drivetrain::ALL,
        &["electric", "combustion", "hybrid", "unknown"],
    );
    assert_wire_names(
        ErrorCategory::ALL,
        &[
            "auth",
            "network",
            "rate_limit",
            "manufacturer",
            "parsing",
            "persistence",
            "configuration",
            "internal",
        ],
    );
}

#[test]
fn unit_names() {
    let units = [
        Unit::Kilometres,
        Unit::Percent,
        Unit::Kilowatts,
        Unit::Celsius,
        Unit::Minutes,
        Unit::Days,
        Unit::Degrees,
        Unit::None,
    ];
    assert_wire_names(
        &units,
        &[
            "kilometres",
            "percent",
            "kilowatts",
            "celsius",
            "minutes",
            "days",
            "degrees",
            "none",
        ],
    );
}
