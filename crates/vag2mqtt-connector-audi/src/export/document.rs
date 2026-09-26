//! Reading one EU Data Act package, and remembering what several packages said.
//!
//! A package is a ZIP holding one JSON document, a flat list of data points
//! `{vin, Data: [{key, dataFieldName, value, timestampUtc}]}`, where `value` is always a
//! string. `fixtures/audi/eudataact/one_time_export.json` is a real, anonymised example. The
//! one-time export arrives as the
//! bare JSON document, so both are accepted.

use std::collections::BTreeMap;
use std::io::Read;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use vag2mqtt_connector_api::ConnectorError;

const LOG: &str = "vag2mqtt::fetch";

/// The largest JSON document a package may unpack to. The real one-time export is under 1 MB; a
/// package a hundred times that size is not a vehicle report.
const MAX_DOCUMENT_BYTES: u64 = 100 * 1024 * 1024;

/// One data point as the package carries it. Every field optional: nothing about this format is
/// documented, and one odd row must not cost the others.
#[derive(Debug, Deserialize)]
struct RawPoint {
    #[serde(rename = "dataFieldName")]
    data_field_name: Option<String>,
    value: Option<String>,
    #[serde(rename = "timestampUtc")]
    timestamp_utc: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawDocument {
    vin: Option<String>,
    #[serde(rename = "Data", alias = "data")]
    data: Option<Vec<RawPoint>>,
}

/// A value and the manufacturer's time for it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Point {
    /// The value, always a string in the package.
    pub(crate) value: String,
    /// When the vehicle captured it, if the package says so in a readable way.
    pub(crate) source_time: Option<DateTime<Utc>>,
}

/// One parsed package: the VIN it names and its points by (stable) field name.
#[derive(Debug, Default)]
pub(crate) struct Document {
    pub(crate) vin: Option<String>,
    pub(crate) points: BTreeMap<String, Point>,
}

/// Reads a package: a ZIP with one `.json` inside, or the JSON document itself.
pub(crate) fn read_package(bytes: &[u8]) -> Result<Document, ConnectorError> {
    let parsing = |context| ConnectorError::Parsing { context };
    let json = if bytes.starts_with(b"PK") {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|_| parsing("export package archive"))?;
        let index = (0..archive.len())
            .find(|&i| {
                archive
                    .by_index(i)
                    .map(|file| file.name().to_ascii_lowercase().ends_with(".json"))
                    .unwrap_or(false)
            })
            .ok_or_else(|| parsing("export package without a json document"))?;
        let file = archive
            .by_index(index)
            .map_err(|_| parsing("export package archive"))?;
        let mut json = Vec::new();
        file.take(MAX_DOCUMENT_BYTES)
            .read_to_end(&mut json)
            .map_err(|_| parsing("export package archive"))?;
        json
    } else {
        bytes.to_vec()
    };
    let raw: RawDocument =
        serde_json::from_slice(&json).map_err(|_| parsing("export package document"))?;
    Ok(document_from(raw))
}

fn document_from(raw: RawDocument) -> Document {
    let rows: Vec<(String, Point)> = raw
        .data
        .unwrap_or_default()
        .into_iter()
        .filter_map(|row| {
            let name = row.data_field_name?;
            let value = row.value.filter(|v| !v.is_empty())?;
            let source_time = row.timestamp_utc.as_deref().and_then(parse_timestamp);
            Some((name, Point { value, source_time }))
        })
        .collect();

    // Envelopes are numbered per package, so `envelope.[2]` means something different in the
    // next one. Naming them by their payload type makes them comparable across packages.
    let payload_types: BTreeMap<String, String> = rows
        .iter()
        .filter_map(|(name, point)| {
            let index = name.strip_prefix("envelope.[")?;
            let (index, rest) = index.split_once(']')?;
            (rest == ".context.payloadType").then(|| (index.to_string(), point.value.clone()))
        })
        .collect();

    let mut points: BTreeMap<String, Point> = BTreeMap::new();
    for (name, point) in rows {
        let name = stable_name(&name, &payload_types);
        match points.get(&name) {
            // The same field several times in one package: the latest capture wins, and among
            // equals the later row.
            Some(existing) if existing.source_time > point.source_time => {}
            _ => {
                points.insert(name, point);
            }
        }
    }
    Document {
        vin: raw.vin,
        points,
    }
}

/// `envelope.[2].report.status` becomes `envelope.CLIMA_STATE_REPORT.report.status`.
fn stable_name(name: &str, payload_types: &BTreeMap<String, String>) -> String {
    if let Some(rest) = name.strip_prefix("envelope.[")
        && let Some((index, tail)) = rest.split_once(']')
        && let Some(kind) = payload_types.get(index)
    {
        return format!("envelope.{kind}{tail}");
    }
    name.to_string()
}

/// Parses every timestamp shape the package uses (reference section 2).
///
/// - RFC 3339 with any number of fraction digits;
/// - `YYYY-MM-DD HH:MM:SS` without a zone, taken as UTC;
/// - a date before 2000, which is how the power curves write **seconds as milliseconds**: the
///   millisecond count is reinterpreted as seconds;
/// - `N/A` and anything else unreadable: `None`, the value is kept without a time.
pub(crate) fn parse_timestamp(text: &str) -> Option<DateTime<Utc>> {
    let text = text.trim();
    let parsed = DateTime::parse_from_rfc3339(text)
        .map(|t| t.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
                .ok()
                .map(|naive| Utc.from_utc_datetime(&naive))
        })?;
    if parsed.timestamp() >= 946_684_800 {
        return Some(parsed);
    }
    let seconds = parsed.timestamp_millis();
    let repaired = Utc.timestamp_opt(seconds, 0).single()?;
    if repaired.timestamp() >= 946_684_800 && repaired <= Utc::now() + chrono::Duration::days(1) {
        Some(repaired)
    } else {
        tracing::debug!(target: LOG, "unreadable export timestamp dropped");
        None
    }
}

/// What the packages seen so far say, field by field. Stored in the session, so it has to stay
/// small: only fields the normaliser reads are kept.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Points {
    by_name: BTreeMap<String, Point>,
}

impl Points {
    /// Takes over every relevant point of a newer package. Packages are merged oldest first, so
    /// a later package always wins, even when its timestamps are odd: the delivery order is the
    /// one thing the portal gets right.
    pub(crate) fn merge(&mut self, document: Document, relevant: impl Fn(&str) -> bool) {
        for (name, point) in document.points {
            if relevant(&name) {
                self.by_name.insert(name, point);
            }
        }
    }

    /// The latest point for a field.
    pub(crate) fn get(&self, name: &str) -> Option<&Point> {
        self.by_name.get(name)
    }

    /// `true` when no package has delivered anything relevant yet.
    pub(crate) fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn fixture() -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/audi/eudataact/one_time_export.json"),
        )
        .unwrap()
    }

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn every_timestamp_shape_of_the_export_is_read() {
        assert_eq!(
            parse_timestamp("2026-09-23T16:06:41.762Z"),
            Some(utc("2026-09-23T16:06:41.762Z"))
        );
        assert_eq!(
            parse_timestamp("2026-09-22T10:22:23.5920Z"),
            Some(utc("2026-09-22T10:22:23.592Z"))
        );
        assert_eq!(
            parse_timestamp("2026-09-21 14:42:54"),
            Some(utc("2026-09-21T14:42:54Z"))
        );
        assert_eq!(
            parse_timestamp("2026-09-23T15:30:00+02:00"),
            Some(utc("2026-09-23T13:30:00Z"))
        );
        assert_eq!(parse_timestamp("N/A"), None);
        assert_eq!(parse_timestamp(""), None);
    }

    #[test]
    fn seconds_written_as_milliseconds_are_repaired() {
        // 1970-01-21T17:03:41.435Z is 1_789_421_435 ms, which is really 1_789_421_435 s.
        assert_eq!(
            parse_timestamp("1970-01-21T17:03:41.435Z"),
            Utc.timestamp_opt(1_789_421_435, 0).single()
        );
        // A 1970 date that does not repair into a plausible time is dropped, not guessed.
        assert_eq!(parse_timestamp("1970-01-01T00:00:01.000Z"), None);
    }

    #[test]
    fn the_fixture_parses_with_stable_envelope_names() {
        let document = read_package(&fixture()).unwrap();
        assert_eq!(document.vin.as_deref(), Some("WAUZZZ0000000TEST"));
        let soc = &document.points["batteryStatus.currentSOC_pct"];
        assert_eq!(soc.value, "51");
        assert_eq!(soc.source_time, Some(utc("2026-09-23T16:06:41.762Z")));
        assert_eq!(
            document.points["envelope.CLIMA_STATE_REPORT.report.status"].value,
            "OFF"
        );
        assert_eq!(
            document.points["envelope.CLIMA_SETTINGS_REPORT.report.targetTemperature.temperature"]
                .value,
            "19.0"
        );
        // Rows without a timestamp keep their value.
        assert_eq!(document.points["INFO_MAKE"].source_time, None);
    }

    #[test]
    fn a_repeated_field_keeps_its_latest_capture() {
        let document = read_package(&fixture()).unwrap();
        // The three trips in the fixture form a chain; the latest one ends furthest.
        let odometer = &document.points["trp.odoE"];
        assert_eq!(odometer.value, "567953.0");
    }

    #[test]
    fn a_zipped_package_reads_like_the_bare_document() {
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            writer
                .start_file(
                    "WAUZZZ0000000TEST_20260924.json",
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            writer.write_all(&fixture()).unwrap();
            writer.finish().unwrap();
        }
        let zipped = read_package(buffer.get_ref()).unwrap();
        let bare = read_package(&fixture()).unwrap();
        assert_eq!(zipped.points, bare.points);
    }

    #[test]
    fn garbage_is_a_parsing_error_not_a_panic() {
        assert!(read_package(b"not json").is_err());
        assert!(read_package(b"PK\x03\x04broken").is_err());
        let empty = read_package(br#"{"vin":"X"}"#).unwrap();
        assert!(empty.points.is_empty());
    }

    #[test]
    fn a_newer_package_wins_and_irrelevant_fields_are_not_kept() {
        let mut points = Points::default();
        let older = read_package(
            br#"{"Data":[{"dataFieldName":"soc","value":"40","timestampUtc":"2026-09-23T10:00:00Z"},
                         {"dataFieldName":"noise","value":"x"}]}"#,
        )
        .unwrap();
        let newer = read_package(
            br#"{"Data":[{"dataFieldName":"soc","value":"41","timestampUtc":"1970-01-01T00:00:00Z"}]}"#,
        )
        .unwrap();
        points.merge(older, |name| name == "soc");
        points.merge(newer, |name| name == "soc");
        assert_eq!(points.get("soc").unwrap().value, "41");
        assert_eq!(points.get("soc").unwrap().source_time, None);
        assert!(points.get("noise").is_none());
    }
}
