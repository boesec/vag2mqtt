//! View models.
//!
//! Templates never see a domain type. Everything they render is built here first, so a change in
//! the domain model breaks this file rather than silently changing a page. Nothing here can hold
//! a [`Secret`](vag2mqtt_domain::Secret): password fields render empty and are filled by the user.

use chrono::{DateTime, Utc};
use serde::Serialize;
use vag2mqtt_domain::state::{DoorPosition, WindowPosition};
use vag2mqtt_domain::units::HasUnit;
use vag2mqtt_domain::{Brand, Reading, VehicleState};
use vag2mqtt_runtime::{AccountStatus, RuntimeStatus, VehicleStatus};

/// What every page needs.
pub(crate) struct Chrome {
    /// The service version, shown in the footer.
    pub(crate) version: &'static str,
    /// Which navigation entry is current.
    pub(crate) active: &'static str,
    /// A message to show at the top, usually after a failed form post. Empty when there is none,
    /// so a template can ask `is_empty()` instead of destructuring an `Option`.
    pub(crate) error: String,
    /// A confirmation to show at the top. Empty when there is none.
    pub(crate) notice: String,
}

impl Chrome {
    /// Chrome for `active`, with nothing to report.
    pub(crate) fn new(active: &'static str) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION"),
            active,
            error: String::new(),
            notice: String::new(),
        }
    }

    /// Chrome carrying an error message.
    pub(crate) fn with_error(active: &'static str, error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            ..Self::new(active)
        }
    }
}

/// A timestamp as the interface shows it: short, UTC, and `—` when absent.
pub(crate) fn moment(time: Option<DateTime<Utc>>) -> String {
    match time {
        Some(time) => time.format("%Y-%m-%d %H:%M:%SZ").to_string(),
        None => "—".to_string(),
    }
}

/// How long ago something happened, for the overview.
pub(crate) fn ago(time: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    let Some(time) = time else {
        return "never".to_string();
    };
    let seconds = (now - time).num_seconds();
    if seconds < 0 {
        return "in a moment".to_string();
    }
    match seconds {
        0..=59 => format!("{seconds} s ago"),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86399 => format!("{} h ago", seconds / 3600),
        _ => format!("{} d ago", seconds / 86400),
    }
}

/// The CSS modifier for a state, so good and bad are distinguishable at a glance.
pub(crate) fn tone(state: &str) -> &'static str {
    match state {
        "ok" | "fresh" | "present" | "online" => "good",
        "pending" | "stale" | "unavailable" | "conservation" => "warn",
        "auth_error" | "rate_limited" | "unreachable" | "error" => "bad",
        "disabled" | "unsupported" => "muted",
        _ => "neutral",
    }
}

/// The serde name of an enum value, which is what the MQTT contract and the API also use.
fn wire_name<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        Ok(other) => other.to_string(),
        Err(_) => "unknown".to_string(),
    }
}

// ----- overview -------------------------------------------------------------------------

/// The part of the overview that htmx refreshes.
pub(crate) struct OverviewFragment {
    /// How long the service has been up.
    pub(crate) started_at: String,
    /// When the snapshot was taken.
    pub(crate) snapshot_at: String,
    /// `0`, `1` or `2`, the same tri-state the MQTT contract publishes.
    pub(crate) connected: &'static str,
    /// What the tri-state means, in words.
    pub(crate) connected_meaning: &'static str,
    /// The CSS tone for the tri-state.
    pub(crate) connected_tone: &'static str,
    /// The broker line.
    pub(crate) mqtt: MqttSummary,
    /// Every account.
    pub(crate) accounts: Vec<AccountRow>,
    /// Every vehicle.
    pub(crate) vehicles: Vec<VehicleRow>,
}

impl OverviewFragment {
    /// Builds the fragment from a runtime snapshot.
    pub(crate) fn new(status: RuntimeStatus) -> Self {
        let now = status.snapshot_at;
        let any_fresh = status
            .vehicles
            .iter()
            .any(|vehicle| wire_name(&vehicle.vehicle.data_state) == "fresh");
        let (connected, meaning, connected_tone) = match (status.mqtt.connected, any_fresh) {
            (false, _) => ("0", "not connected to the broker", "bad"),
            (true, false) => ("1", "connected, no account delivering fresh data", "warn"),
            (true, true) => ("2", "connected and delivering", "good"),
        };
        Self {
            started_at: moment(Some(status.started_at)),
            snapshot_at: moment(Some(status.snapshot_at)),
            connected,
            connected_meaning: meaning,
            connected_tone,
            mqtt: MqttSummary::new(&status),
            accounts: status
                .accounts
                .iter()
                .map(|account| AccountRow::new(account, now))
                .collect(),
            vehicles: status
                .vehicles
                .iter()
                .map(|vehicle| VehicleRow::new(vehicle, now))
                .collect(),
        }
    }
}

/// The broker, in one line.
pub(crate) struct MqttSummary {
    /// Whether anything is configured.
    pub(crate) configured: bool,
    /// `host:port`, or a hint when nothing is configured.
    pub(crate) target: String,
    /// The topic prefix.
    pub(crate) prefix: String,
    /// Publishing is switched on.
    pub(crate) enabled: bool,
    /// A live broker session.
    pub(crate) connected: bool,
}

impl MqttSummary {
    fn new(status: &RuntimeStatus) -> Self {
        match &status.mqtt.config {
            Some(config) => Self {
                configured: true,
                target: format!("{}:{}", config.host, config.port),
                prefix: config.topic_prefix.clone(),
                enabled: status.mqtt.enabled,
                connected: status.mqtt.connected,
            },
            None => Self {
                configured: false,
                target: "no broker configured".to_string(),
                prefix: "—".to_string(),
                enabled: false,
                connected: false,
            },
        }
    }
}

/// One account in a list.
pub(crate) struct AccountRow {
    /// Its identifier, used in links.
    pub(crate) id: String,
    /// The brand, as a word.
    pub(crate) brand: String,
    /// The login identifier.
    pub(crate) username: String,
    /// The connection state, as a word.
    pub(crate) state: String,
    /// The CSS tone for that state.
    pub(crate) tone: &'static str,
    /// When the manufacturer last answered.
    pub(crate) last_success: String,
    /// When the next poll is due.
    pub(crate) next_poll: String,
    /// Whether the user switched it on.
    pub(crate) enabled: bool,
    /// The last error's message, empty when there is none.
    pub(crate) error: String,
}

impl AccountRow {
    /// Builds a row from the runtime snapshot.
    pub(crate) fn new(status: &AccountStatus, now: DateTime<Utc>) -> Self {
        let state = wire_name(&status.account.connection_state);
        Self {
            id: status.account.id.to_string(),
            brand: wire_name(&status.account.brand),
            username: status.account.username.clone(),
            tone: tone(&state),
            state,
            last_success: ago(status.account.last_success_at, now),
            next_poll: match status.next_poll_at {
                Some(at) if at > now => format!("in {}", ago(Some(now - (at - now)), now)),
                Some(_) => "due".to_string(),
                None => "—".to_string(),
            },
            enabled: status.account.enabled,
            error: status
                .account
                .last_error
                .as_ref()
                .map(|error| error.message.clone())
                .unwrap_or_default(),
        }
    }
}

/// One vehicle in a list.
pub(crate) struct VehicleRow {
    /// The VIN, used in links.
    pub(crate) vin: String,
    /// What to call it.
    pub(crate) label: String,
    /// The data state, as a word.
    pub(crate) state: String,
    /// The CSS tone for that state.
    pub(crate) tone: &'static str,
    /// When it was last updated.
    pub(crate) last_update: String,
    /// Whether the user switched it on.
    pub(crate) enabled: bool,
    /// Set when the manufacturer stopped listing it.
    pub(crate) missing: bool,
}

impl VehicleRow {
    /// Builds a row from the runtime snapshot.
    pub(crate) fn new(status: &VehicleStatus, now: DateTime<Utc>) -> Self {
        let state = wire_name(&status.vehicle.data_state);
        Self {
            vin: status.vehicle.vin.to_string(),
            label: status.vehicle.label().to_string(),
            tone: tone(&state),
            state,
            last_update: ago(status.vehicle.last_update_at, now),
            enabled: status.vehicle.enabled,
            missing: status.vehicle.missing_since.is_some(),
        }
    }
}

// ----- forms ----------------------------------------------------------------------------

/// The broker form.
pub(crate) struct MqttView {
    /// Page chrome.
    pub(crate) chrome: Chrome,
    /// Broker host.
    pub(crate) host: String,
    /// Broker port.
    pub(crate) port: u16,
    /// Broker user name.
    pub(crate) username: String,
    /// Whether a configuration already exists, which changes the password hint.
    pub(crate) configured: bool,
    /// Connect with TLS.
    pub(crate) tls: bool,
    /// The topic prefix.
    pub(crate) topic_prefix: String,
    /// The client identifier.
    pub(crate) client_id: String,
    /// Keep alive in seconds.
    pub(crate) keep_alive_secs: u64,
    /// Publishing switched on.
    pub(crate) enabled: bool,
    /// A live broker session right now.
    pub(crate) connected: bool,
}

impl MqttView {
    /// Builds the form from the current configuration, or empty defaults.
    pub(crate) fn new(chrome: Chrome, status: &RuntimeStatus) -> Self {
        match &status.mqtt.config {
            Some(config) => Self {
                chrome,
                host: config.host.clone(),
                port: config.port,
                username: config.username.clone().unwrap_or_default(),
                configured: true,
                tls: config.tls,
                topic_prefix: config.topic_prefix.clone(),
                client_id: config.client_id.clone(),
                keep_alive_secs: config.keep_alive.as_secs(),
                enabled: status.mqtt.enabled,
                connected: status.mqtt.connected,
            },
            None => Self {
                chrome,
                host: String::new(),
                port: 1883,
                username: String::new(),
                configured: false,
                tls: false,
                topic_prefix: "vag2mqtt".to_string(),
                client_id: String::new(),
                keep_alive_secs: 30,
                enabled: true,
                connected: false,
            },
        }
    }
}

/// One entry of the brand dropdown.
pub(crate) struct BrandOption {
    /// The value posted back.
    pub(crate) value: String,
    /// What the user sees.
    pub(crate) label: String,
}

impl BrandOption {
    /// Builds the options from the registry's brands.
    pub(crate) fn list(brands: &[Brand]) -> Vec<Self> {
        brands
            .iter()
            .map(|brand| Self {
                value: wire_name(brand),
                label: {
                    let name = wire_name(brand);
                    let mut chars = name.chars();
                    match chars.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                        None => name,
                    }
                },
            })
            .collect()
    }
}

/// The last error, shown with its category, message and time.
pub(crate) struct ErrorDetail {
    /// When it happened.
    pub(crate) at: String,
    /// Its category.
    pub(crate) category: String,
    /// The message.
    pub(crate) message: String,
}

impl ErrorDetail {
    /// Builds the detail from a domain error, or a blank one when there is none.
    pub(crate) fn new(error: Option<&vag2mqtt_domain::LastError>) -> Self {
        match error {
            Some(error) => Self {
                at: moment(Some(error.at)),
                category: wire_name(&error.category),
                message: error.message.clone(),
            },
            None => Self {
                at: "—".to_string(),
                category: String::new(),
                message: String::new(),
            },
        }
    }
}

// ----- diagnostics ----------------------------------------------------------------------

/// One line of the diagnostics table.
pub(crate) struct DiagnosticRow {
    /// The contract path, for example `battery/soc`.
    pub(crate) path: String,
    /// `present`, `unavailable` or `unsupported`.
    pub(crate) state: &'static str,
    /// The CSS tone for that state.
    pub(crate) tone: &'static str,
    /// The value, or `—`.
    pub(crate) value: String,
    /// The unit symbol, or empty.
    pub(crate) unit: &'static str,
    /// The manufacturer's timestamp for the value, or `—`.
    pub(crate) source_time: String,
}

/// Builds one row from a reading of a value type that knows its unit.
fn row<T: Serialize + HasUnit>(path: &str, reading: &Reading<T>) -> DiagnosticRow {
    let (state, value, source_time) = match reading {
        Reading::Unsupported => ("unsupported", "—".to_string(), "—".to_string()),
        Reading::Unavailable => ("unavailable", "—".to_string(), "—".to_string()),
        Reading::Present(sample) => (
            "present",
            render_value(&sample.value),
            moment(sample.source_time),
        ),
    };
    DiagnosticRow {
        path: path.to_string(),
        state,
        tone: tone(state),
        value,
        unit: if state == "present" {
            T::UNIT.symbol()
        } else {
            ""
        },
        source_time,
    }
}

/// A value as the table shows it: a bare number, or an enum's wire name.
fn render_value<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(serde_json::Value::Bool(flag)) => flag.to_string(),
        Ok(serde_json::Value::Number(number)) => number.to_string(),
        Ok(other) => other.to_string(),
        Err(_) => "?".to_string(),
    }
}

/// Position gets its own row: the state and the source time are what a first live test needs,
/// and the coordinates themselves are the one thing the project never puts on a screen or in a
/// log.
fn position_row(state: &VehicleState) -> DiagnosticRow {
    let (label, value, source_time) = match &state.position {
        Reading::Unsupported => ("unsupported", "—".to_string(), "—".to_string()),
        Reading::Unavailable => ("unavailable", "—".to_string(), "—".to_string()),
        Reading::Present(sample) => (
            "present",
            "(withheld)".to_string(),
            moment(sample.source_time),
        ),
    };
    DiagnosticRow {
        path: "position".to_string(),
        state: label,
        tone: tone(label),
        value,
        unit: "",
        source_time,
    }
}

/// Every category of the snapshot, in the order of the MQTT contract.
pub(crate) fn diagnostic_rows(state: &VehicleState) -> Vec<DiagnosticRow> {
    let mut rows = vec![
        row("odometer", &state.odometer),
        row("battery/soc", &state.battery.soc),
        row("battery/range", &state.battery.range),
        row("fuel/level", &state.fuel.level),
        row("fuel/range", &state.fuel.range),
        row("charging/state", &state.charging.state),
        row("charging/mode", &state.charging.mode),
        row("charging/current_type", &state.charging.current_type),
        row("charging/power", &state.charging.power),
        row("charging/remaining_time", &state.charging.remaining_time),
        row("charging/target_soc", &state.charging.target_soc),
        row("plug/connection", &state.plug.connection),
        row("plug/lock", &state.plug.lock),
        row("plug/external_power", &state.plug.external_power),
        row("doors/open", &state.doors.overall),
        row("doors/lock", &state.doors.lock),
    ];
    for position in DoorPosition::ALL {
        let reading = state
            .doors
            .by_position
            .get(position)
            .cloned()
            .unwrap_or(Reading::Unsupported);
        rows.push(row(
            &format!("doors/{}/open", wire_name(position)),
            &reading,
        ));
    }
    rows.push(row("windows/open", &state.windows.overall));
    for position in WindowPosition::ALL {
        let reading = state
            .windows
            .by_position
            .get(position)
            .cloned()
            .unwrap_or(Reading::Unsupported);
        rows.push(row(
            &format!("windows/{}/open", wire_name(position)),
            &reading,
        ));
    }
    rows.push(position_row(state));
    rows.push(row("climatisation/state", &state.climatisation.state));
    rows.push(row(
        "climatisation/target_temperature",
        &state.climatisation.target_temperature,
    ));
    rows.push(row(
        "climatisation/remaining_time",
        &state.climatisation.remaining_time,
    ));
    rows.push(row("status/connection", &state.status.connection));
    rows.push(row("status/activity", &state.status.activity));
    rows.push(row("status/secured", &state.status.secured));
    rows.push(row(
        "status/outside_temperature",
        &state.status.outside_temperature,
    ));
    rows.push(row(
        "service/inspection_due_days",
        &state.service.inspection_due_in,
    ));
    rows.push(row(
        "service/inspection_due_km",
        &state.service.inspection_due_after,
    ));
    rows.push(row(
        "service/oil_service_due_days",
        &state.service.oil_service_due_in,
    ));
    rows.push(row(
        "service/oil_service_due_km",
        &state.service.oil_service_due_after,
    ));
    rows
}

/// The handful of values worth showing on the vehicle page itself.
pub(crate) fn highlight_rows(state: &VehicleState) -> Vec<DiagnosticRow> {
    vec![
        row("odometer", &state.odometer),
        row("battery/soc", &state.battery.soc),
        row("battery/range", &state.battery.range),
        row("charging/state", &state.charging.state),
        row("plug/connection", &state.plug.connection),
        row("status/connection", &state.status.connection),
    ]
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use vag2mqtt_domain::state::{ChargingState, OpenState};
    use vag2mqtt_domain::units::{Kilometres, Percent};

    use super::*;

    fn at(hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 24, hour, 0, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn the_three_states_are_told_apart() {
        let mut state = VehicleState::unsupported(at(12));
        state.odometer = Reading::present(Kilometres::new(43120), Some(at(8)));
        state.battery.soc = Reading::present(Percent::try_new(67).unwrap(), Some(at(8)));
        state.climatisation.state = Reading::Unavailable;

        let rows = diagnostic_rows(&state);
        let find = |path: &str| {
            rows.iter()
                .find(|row| row.path == path)
                .unwrap_or_else(|| panic!("{path} is missing"))
        };

        let odometer = find("odometer");
        assert_eq!(odometer.state, "present");
        assert_eq!(odometer.value, "43120");
        assert_eq!(odometer.unit, "km");
        assert_eq!(odometer.source_time, "2026-09-24 08:00:00Z");
        assert_eq!(odometer.tone, "good");

        let soc = find("battery/soc");
        assert_eq!(soc.value, "67");
        assert_eq!(soc.unit, "%");

        let climatisation = find("climatisation/state");
        assert_eq!(climatisation.state, "unavailable");
        assert_eq!(climatisation.value, "—");
        assert_eq!(climatisation.unit, "");
        assert_eq!(climatisation.tone, "warn");

        let fuel = find("fuel/range");
        assert_eq!(fuel.state, "unsupported");
        assert_eq!(fuel.tone, "muted");
        assert_ne!(
            fuel.tone, climatisation.tone,
            "unsupported and unavailable must look different"
        );
    }

    /// The page shows exactly what the MQTT contract publishes, so a category that reaches a
    /// broker can never be missing from the diagnosis, nor the other way round.
    #[test]
    fn the_page_covers_exactly_the_contract() {
        let rows = diagnostic_rows(&VehicleState::unsupported(at(12)));
        let shown: std::collections::BTreeSet<String> = rows
            .iter()
            .map(|row| vag2mqtt_mqtt::pattern_of(&row.path))
            .collect();

        let mut expected: std::collections::BTreeSet<String> = vag2mqtt_mqtt::PATH_PATTERNS
            .iter()
            .map(|path| path.to_string())
            .collect();
        // The three position topics become one row: the page reports that a position exists
        // without printing the coordinates.
        expected.remove("position/latitude");
        expected.remove("position/longitude");
        expected.remove("position/heading");
        expected.insert("position".to_string());

        assert_eq!(shown, expected);

        // Every door and every window is a row of its own, not a pattern.
        assert_eq!(
            rows.len(),
            40,
            "28 fixed categories, 6 doors, 5 windows and 1 position"
        );
        for path in ["doors/bonnet/open", "windows/sun_roof/open", "position"] {
            assert!(rows.iter().any(|row| row.path == path), "{path}");
        }
    }

    #[test]
    fn coordinates_are_never_rendered() {
        let mut state = VehicleState::unsupported(at(12));
        state.position = Reading::present(
            vag2mqtt_domain::state::GeoPosition::try_new(48.137154, 11.576124, None).unwrap(),
            Some(at(7)),
        );
        let rows = diagnostic_rows(&state);
        let position = rows.iter().find(|row| row.path == "position").unwrap();
        assert_eq!(position.state, "present");
        assert_eq!(position.source_time, "2026-09-24 07:00:00Z");
        for row in &rows {
            assert!(
                !row.value.contains("48.13"),
                "a coordinate reached the page"
            );
            assert!(
                !row.value.contains("11.57"),
                "a coordinate reached the page"
            );
        }
    }

    #[test]
    fn enum_values_render_as_their_wire_name() {
        let mut state = VehicleState::unsupported(at(12));
        state.charging.state = Reading::present(ChargingState::ReadyForCharging, None);
        state.doors.overall = Reading::present(OpenState::Ajar, None);
        let rows = diagnostic_rows(&state);
        assert_eq!(
            rows.iter()
                .find(|r| r.path == "charging/state")
                .unwrap()
                .value,
            "ready_for_charging"
        );
        assert_eq!(
            rows.iter().find(|r| r.path == "doors/open").unwrap().value,
            "ajar"
        );
    }

    #[test]
    fn relative_times_read_naturally() {
        let now = at(12);
        assert_eq!(ago(None, now), "never");
        assert_eq!(ago(Some(now), now), "0 s ago");
        assert_eq!(
            ago(Some(now - chrono::Duration::seconds(30)), now),
            "30 s ago"
        );
        assert_eq!(
            ago(Some(now - chrono::Duration::minutes(5)), now),
            "5 min ago"
        );
        assert_eq!(ago(Some(now - chrono::Duration::hours(3)), now), "3 h ago");
        assert_eq!(ago(Some(now - chrono::Duration::days(2)), now), "2 d ago");
        assert_eq!(moment(None), "—");
    }
}
