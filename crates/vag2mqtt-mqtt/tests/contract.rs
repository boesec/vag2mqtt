//! Keeps `Docs/mqtt-contract.md` and the flattener in sync, and pins the flattening of the
//! WP-01 fixture.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::PathBuf;

use chrono::{TimeZone, Utc};
use vag2mqtt_domain::state::{
    ChargeCurrentType, ChargingMode, ChargingState, ClimatisationState, DoorPosition,
    ExternalPower, GeoPosition, LockState, OpenState, PlugConnection, VehicleActivity,
    VehicleConnection, WindowPosition,
};
use vag2mqtt_domain::units::{
    Celsius, Days, Kilometres, Kilowatts, Minutes, Percent, ServiceKilometres,
};
use vag2mqtt_domain::{Reading, VehicleState};
use vag2mqtt_mqtt::flatten;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn contract_doc_vehicle_paths() -> BTreeSet<String> {
    let doc = std::fs::read_to_string(manifest_dir().join("../../Docs/mqtt-contract.md")).unwrap();
    let mut in_vehicle_table = false;
    let mut paths = BTreeSet::new();
    for line in doc.lines() {
        if line.starts_with("## Vehicle topics") {
            in_vehicle_table = true;
            continue;
        }
        if in_vehicle_table && line.starts_with("### ") {
            break;
        }
        if in_vehicle_table && line.starts_with("| `") {
            let path = line
                .trim_start_matches("| `")
                .split('`')
                .next()
                .unwrap()
                .to_string();
            paths.insert(path);
        }
    }
    paths
}

/// A snapshot where every category, every door, every window and the heading are present.
fn everything_present() -> VehicleState {
    let fetched_at = Utc
        .with_ymd_and_hms(2026, 9, 22, 10, 30, 0)
        .single()
        .unwrap();
    let mut s = VehicleState::unsupported(fetched_at);
    let pct = |v| Percent::try_new(v).unwrap();
    s.odometer = Reading::present(Kilometres::new(1), None);
    s.battery.soc = Reading::present(pct(1), None);
    s.battery.range = Reading::present(Kilometres::new(1), None);
    s.fuel.level = Reading::present(pct(1), None);
    s.fuel.range = Reading::present(Kilometres::new(1), None);
    s.charging.state = Reading::present(ChargingState::Off, None);
    s.charging.mode = Reading::present(ChargingMode::Off, None);
    s.charging.current_type = Reading::present(ChargeCurrentType::Ac, None);
    s.charging.power = Reading::present(Kilowatts::new(1.0), None);
    s.charging.remaining_time = Reading::present(Minutes::new(1), None);
    s.charging.target_soc = Reading::present(pct(1), None);
    s.plug.connection = Reading::present(PlugConnection::Connected, None);
    s.plug.lock = Reading::present(LockState::Locked, None);
    s.plug.external_power = Reading::present(ExternalPower::Active, None);
    s.doors.overall = Reading::present(OpenState::Closed, None);
    s.doors.lock = Reading::present(LockState::Locked, None);
    for p in DoorPosition::ALL {
        s.doors
            .by_position
            .insert(*p, Reading::present(OpenState::Closed, None));
    }
    s.windows.overall = Reading::present(OpenState::Closed, None);
    for p in WindowPosition::ALL {
        s.windows
            .by_position
            .insert(*p, Reading::present(OpenState::Closed, None));
    }
    s.position = Reading::present(GeoPosition::try_new(1.0, 1.0, Some(1.0)).unwrap(), None);
    s.climatisation.state = Reading::present(ClimatisationState::Off, None);
    s.climatisation.target_temperature = Reading::present(Celsius::new(1.0), None);
    s.climatisation.remaining_time = Reading::present(Minutes::new(1), None);
    s.status.connection = Reading::present(VehicleConnection::Online, None);
    s.status.activity = Reading::present(VehicleActivity::Parked, None);
    s.status.secured = Reading::present(true, None);
    s.status.outside_temperature = Reading::present(Celsius::new(1.0), None);
    s.service.inspection_due_in = Reading::present(Days::new(1), None);
    s.service.inspection_due_after = Reading::present(ServiceKilometres::new(1), None);
    s.service.oil_service_due_in = Reading::present(Days::new(1), None);
    s.service.oil_service_due_after = Reading::present(ServiceKilometres::new(1), None);
    s
}

#[test]
fn contract_doc_and_flattener_agree() {
    let mut doc_paths = contract_doc_vehicle_paths();
    assert!(doc_paths.remove("availability"), "doc lists availability");
    assert!(doc_paths.remove("full"), "doc lists full");

    let emitted: BTreeSet<String> = flatten(&everything_present())
        .iter()
        .map(|v| vag2mqtt_mqtt::pattern_of(&v.path))
        .collect();
    let declared: BTreeSet<String> = vag2mqtt_mqtt::PATH_PATTERNS
        .iter()
        .map(|p| p.to_string())
        .collect();

    assert_eq!(
        emitted, declared,
        "PATH_PATTERNS and the flattener disagree"
    );
    assert_eq!(
        doc_paths, declared,
        "Docs/mqtt-contract.md and the flattener disagree"
    );
}

#[test]
fn every_position_has_its_own_topic() {
    let flat = flatten(&everything_present());
    let paths: BTreeSet<&str> = flat.iter().map(|v| v.path.as_str()).collect();
    for segment in [
        "front_left",
        "front_right",
        "rear_left",
        "rear_right",
        "trunk",
        "bonnet",
    ] {
        assert!(paths.contains(format!("doors/{segment}/open").as_str()));
    }
    for segment in [
        "front_left",
        "front_right",
        "rear_left",
        "rear_right",
        "sun_roof",
    ] {
        assert!(paths.contains(format!("windows/{segment}/open").as_str()));
    }
}

fn domain_fixture() -> VehicleState {
    let path = manifest_dir().join("../vag2mqtt-domain/tests/fixtures/vehicle_state_full.json");
    let text = std::fs::read_to_string(path).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn topics_fixture_path() -> PathBuf {
    manifest_dir()
        .join("tests")
        .join("fixtures")
        .join("full_state_topics.json")
}

fn flattened_fixture_as_json() -> serde_json::Value {
    let flat = flatten(&domain_fixture());
    serde_json::Value::Array(
        flat.iter()
            .map(|v| {
                serde_json::json!({
                    "path": v.path,
                    "val": v.val,
                    "ts": v.ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                })
            })
            .collect(),
    )
}

#[test]
fn flattening_of_the_domain_fixture_is_pinned() {
    let expected: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(topics_fixture_path()).unwrap()).unwrap();
    let actual = flattened_fixture_as_json();
    if expected != actual {
        panic!(
            "flattening changed. If intended, run\n\
             cargo test -p vag2mqtt-mqtt --test contract -- --ignored write_topics_fixture\n\
             and review the diff.\n\nactual:\n{}",
            serde_json::to_string_pretty(&actual).unwrap()
        );
    }
    // The fixture has Bonnet unsupported and the sun roof unavailable: neither may appear.
    let paths: Vec<&str> = actual
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["path"].as_str().unwrap())
        .collect();
    assert!(!paths.contains(&"doors/bonnet/open"));
    assert!(!paths.contains(&"windows/sun_roof/open"));
    assert!(!paths.contains(&"fuel/level"));
    assert!(!paths.contains(&"climatisation/remaining_time"));
}

#[test]
#[ignore = "regenerates the fixture; run on purpose"]
fn write_topics_fixture() {
    let pretty = serde_json::to_string_pretty(&flattened_fixture_as_json()).unwrap();
    std::fs::write(topics_fixture_path(), format!("{pretty}\n")).unwrap();
}
