//! From export points to the brand-independent [`VehicleState`].
//!
//! Every mapping is a field name constant below, checked against the fixture. The rule for
//! the three states:
//!
//! - a category the export never carries (doors, windows, position, service, vehicle status,
//!   remaining times, fuel on an electric car) stays **unsupported**;
//! - a mapped field that is missing or unreadable in what has arrived so far is **unavailable**;
//! - everything else is **present**, with the manufacturer's time where the package has one.

use chrono::{DateTime, Utc};
use vag2mqtt_domain::state::{
    ChargeCurrentType, ChargingMode, ChargingState, ClimatisationState, ExternalPower, LockState,
    PlugConnection,
};
use vag2mqtt_domain::units::{Celsius, Kilometres, Kilowatts, Percent};
use vag2mqtt_domain::{Reading, VehicleState};

use super::document::Points;

const LOG: &str = "vag2mqtt::fetch";

const ODOMETER_METRES: &str = "trp.odoE";
const SOC: &str = "batteryStatus.currentSOC_pct";
const RANGE: &str = "batteryStatus.cruisingRange.range";
const CHARGE_STATE: &str = "chargingStatus.currentChargeState";
const CHARGE_MODE: &str = "chargeModeSelection";
const CHARGE_TYPE: &str = "chargingStatus.chargeType";
const CHARGE_POWER: &str = "chargingStatus.chargePower_kW";
const CHARGE_LIMIT: &str = "state.threshold";
const PLUG_CONNECTION: &str = "plugStatusItem.plugConnectionState";
const PLUG_LOCK: &str = "plugStatusItem.plugLockState";
const PLUG_POWER: &str = "plugStatusItem.infrastructureState";
const CLIMA_STATE: &str = "envelope.CLIMA_STATE_REPORT.report.status";
const CLIMA_TARGET: &str = "envelope.CLIMA_SETTINGS_REPORT.report.targetTemperature.temperature";
const CLIMA_TARGET_UNIT: &str = "envelope.CLIMA_SETTINGS_REPORT.report.targetTemperature.unit";

/// Every field the normaliser reads. Only these are kept between packages.
pub(crate) const FIELDS: &[&str] = &[
    ODOMETER_METRES,
    SOC,
    RANGE,
    CHARGE_STATE,
    CHARGE_MODE,
    CHARGE_TYPE,
    CHARGE_POWER,
    CHARGE_LIMIT,
    PLUG_CONNECTION,
    PLUG_LOCK,
    PLUG_POWER,
    CLIMA_STATE,
    CLIMA_TARGET,
    CLIMA_TARGET_UNIT,
];

/// `true` for a field the normaliser reads.
pub(crate) fn is_relevant(name: &str) -> bool {
    FIELDS.contains(&name)
}

/// Builds the state from what the packages have delivered so far.
pub(crate) fn normalise(points: &Points, fetched_at: DateTime<Utc>) -> VehicleState {
    let mut state = VehicleState::unsupported(fetched_at);
    let read = Reader { points };

    state.odometer = read.field(ODOMETER_METRES, |v| {
        let metres = number(v)?;
        (0.0..f64::from(u32::MAX))
            .contains(&metres)
            .then(|| Kilometres::new((metres / 1000.0).round() as u32))
    });

    state.battery.soc = read.field(SOC, percent);
    state.battery.range = read.field(RANGE, |v| {
        let km = number(v)?;
        (0.0..10_000.0)
            .contains(&km)
            .then(|| Kilometres::new(km.round() as u32))
    });

    state.charging.state = read.field(CHARGE_STATE, |v| Some(charging_state(v)));
    state.charging.mode = read.field(CHARGE_MODE, |v| Some(charging_mode(v)));
    state.charging.current_type = read.field(CHARGE_TYPE, |v| {
        Some(match token(v).as_str() {
            "ac" => ChargeCurrentType::Ac,
            "dc" => ChargeCurrentType::Dc,
            _ => ChargeCurrentType::Unknown,
        })
    });
    state.charging.power = read.field(CHARGE_POWER, |v| {
        let kw = number(v)?;
        (kw >= 0.0).then(|| Kilowatts::new(kw))
    });
    // Confirmed by the user on 2026-09-24: the car's charge limit is 80 %, and this is the
    // field that carries it, although it arrives inside a battery care notice.
    state.charging.target_soc = read.field(CHARGE_LIMIT, percent);

    state.plug.connection = read.field(PLUG_CONNECTION, |v| {
        Some(match token(v).as_str() {
            "connected" => PlugConnection::Connected,
            "disconnected" => PlugConnection::Disconnected,
            _ => PlugConnection::Unknown,
        })
    });
    state.plug.lock = read.field(PLUG_LOCK, |v| Some(lock_state(v)));
    state.plug.external_power = read.field(PLUG_POWER, |v| {
        Some(match token(v).as_str() {
            "available" | "ready" => ExternalPower::Available,
            "active" | "charging" => ExternalPower::Active,
            "unavailable" => ExternalPower::Unavailable,
            _ => ExternalPower::Unknown,
        })
    });

    state.climatisation.state = read.field(CLIMA_STATE, |v| {
        Some(match token(v).as_str() {
            "off" | "inactive" => ClimatisationState::Off,
            "heating" => ClimatisationState::Heating,
            "cooling" => ClimatisationState::Cooling,
            "ventilation" => ClimatisationState::Ventilation,
            _ => ClimatisationState::Unknown,
        })
    });
    let unit = points.get(CLIMA_TARGET_UNIT).map(|p| token(&p.value));
    state.climatisation.target_temperature = read.field(CLIMA_TARGET, |v| {
        let value = number(v)?;
        let celsius = match unit.as_deref() {
            None | Some("celsius") => value,
            Some("fahrenheit") => (value - 32.0) * 5.0 / 9.0,
            Some(_) => return None,
        };
        (-40.0..=60.0)
            .contains(&celsius)
            .then(|| Celsius::new(celsius))
    });

    state
}

struct Reader<'a> {
    points: &'a Points,
}

impl Reader<'_> {
    /// A mapped field: present when it arrived and parses, unavailable otherwise.
    fn field<T>(&self, name: &str, parse: impl FnOnce(&str) -> Option<T>) -> Reading<T> {
        let Some(point) = self.points.get(name) else {
            return Reading::Unavailable;
        };
        match parse(&point.value) {
            Some(value) => Reading::present(value, point.source_time),
            None => {
                tracing::debug!(target: LOG, field = name, "export value could not be read");
                Reading::Unavailable
            }
        }
    }
}

fn number(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn percent(value: &str) -> Option<Percent> {
    let number = number(value)?;
    if !(0.0..=100.0).contains(&number) {
        return None;
    }
    Percent::try_new(number.round() as u8).ok()
}

/// `NOT_READY_FOR_CHARGING`, `notReadyForCharging` and `not-ready-for-charging` all become
/// `notreadyforcharging`, so the mappings do not care how a catalogue spells its enums.
fn token(value: &str) -> String {
    value
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

fn charging_state(value: &str) -> ChargingState {
    match token(value).as_str() {
        "notreadyforcharging" | "off" => ChargingState::Off,
        "readyforcharging" | "chargepurposereachedandnotconservationcharging" => {
            ChargingState::ReadyForCharging
        }
        "charging" => ChargingState::Charging,
        "conservation" | "chargepurposereachedandconservation" => ChargingState::Conservation,
        "discharging" => ChargingState::Discharging,
        "error" => ChargingState::Error,
        _ => ChargingState::Unknown,
    }
}

fn charging_mode(value: &str) -> ChargingMode {
    match token(value).as_str() {
        "manual" | "immediately" | "immediatelyprofile" => ChargingMode::Manual,
        "timer" | "timers" => ChargingMode::Timer,
        "preferredchargingtimes" | "preferredtimes" => ChargingMode::PreferredTimes,
        "off" => ChargingMode::Off,
        _ => ChargingMode::Unknown,
    }
}

fn lock_state(value: &str) -> LockState {
    match token(value).as_str() {
        "locked" => LockState::Locked,
        "unlocked" => LockState::Unlocked,
        _ => LockState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use vag2mqtt_domain::Sample;

    use super::*;
    use crate::export::document::read_package;

    fn fixture_points() -> Points {
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/audi/eudataact/one_time_export.json"),
        )
        .unwrap();
        let mut points = Points::default();
        points.merge(read_package(&bytes).unwrap(), is_relevant);
        points
    }

    fn value<T: Clone>(reading: &Reading<T>) -> T {
        match reading {
            Reading::Present(Sample { value, .. }) => value.clone(),
            other => panic!("expected a present value, got {:?}", other.is_supported()),
        }
    }

    #[test]
    fn the_fixture_fills_every_mapped_field() {
        let state = normalise(&fixture_points(), Utc::now());
        assert_eq!(value(&state.odometer), Kilometres::new(568));
        assert_eq!(value(&state.battery.soc), Percent::try_new(51).unwrap());
        assert_eq!(value(&state.battery.range), Kilometres::new(206));
        assert_eq!(value(&state.charging.state), ChargingState::Off);
        assert_eq!(value(&state.charging.mode), ChargingMode::Manual);
        assert_eq!(value(&state.charging.current_type), ChargeCurrentType::Ac);
        assert_eq!(value(&state.charging.power), Kilowatts::new(0.0));
        assert_eq!(
            value(&state.charging.target_soc),
            Percent::try_new(80).unwrap()
        );
        assert_eq!(value(&state.plug.connection), PlugConnection::Disconnected);
        assert_eq!(value(&state.plug.lock), LockState::Unlocked);
        assert_eq!(
            value(&state.plug.external_power),
            ExternalPower::Unavailable
        );
        assert_eq!(value(&state.climatisation.state), ClimatisationState::Off);
        assert_eq!(
            value(&state.climatisation.target_temperature),
            Celsius::new(19.0)
        );
    }

    #[test]
    fn values_carry_the_manufacturer_time() {
        let state = normalise(&fixture_points(), Utc::now());
        let Reading::Present(soc) = &state.battery.soc else {
            panic!("soc missing");
        };
        assert_eq!(
            soc.source_time.map(|t| t.to_rfc3339()),
            Some("2026-09-23T16:06:41.762+00:00".to_string())
        );
    }

    #[test]
    fn what_the_export_never_carries_stays_unsupported() {
        let state = normalise(&fixture_points(), Utc::now());
        assert!(!state.position.is_supported(), "the export has no GPS");
        assert!(!state.doors.lock.is_supported());
        assert!(!state.windows.overall.is_supported());
        assert!(!state.fuel.level.is_supported());
        assert!(!state.charging.remaining_time.is_supported());
        assert!(!state.status.connection.is_supported());
        assert!(!state.service.inspection_due_in.is_supported());
    }

    #[test]
    fn nothing_delivered_yet_is_unavailable_not_invented() {
        let state = normalise(&Points::default(), Utc::now());
        assert!(matches!(state.battery.soc, Reading::Unavailable));
        assert!(matches!(state.odometer, Reading::Unavailable));
        assert!(matches!(state.climatisation.state, Reading::Unavailable));
        assert!(!state.position.is_supported());
    }

    #[test]
    fn odd_values_become_unavailable_or_unknown_rather_than_wrong() {
        let mut points = Points::default();
        let document = read_package(
            br#"{"Data":[
                {"dataFieldName":"batteryStatus.currentSOC_pct","value":"180"},
                {"dataFieldName":"chargingStatus.currentChargeState","value":"SOMETHING_NEW"},
                {"dataFieldName":"envelope.[0].context.payloadType","value":"CLIMA_SETTINGS_REPORT"},
                {"dataFieldName":"envelope.[0].report.targetTemperature.temperature","value":"70"},
                {"dataFieldName":"envelope.[0].report.targetTemperature.unit","value":"FAHRENHEIT"}
            ]}"#,
        )
        .unwrap();
        points.merge(document, is_relevant);
        let state = normalise(&points, Utc::now());
        assert!(matches!(state.battery.soc, Reading::Unavailable));
        assert_eq!(value(&state.charging.state), ChargingState::Unknown);
        let celsius = value(&state.climatisation.target_temperature).value();
        assert!((celsius - 21.111).abs() < 0.01, "{celsius}");
    }

    #[test]
    fn enum_spellings_do_not_matter() {
        assert_eq!(charging_state("notReadyForCharging"), ChargingState::Off);
        assert_eq!(charging_state("CHARGING"), ChargingState::Charging);
        assert_eq!(
            charging_state("chargePurposeReachedAndConservation"),
            ChargingState::Conservation
        );
        assert_eq!(charging_mode("IMMEDIATELY_PROFILE"), ChargingMode::Manual);
        assert_eq!(lock_state("LOCKED"), LockState::Locked);
    }
}
