//! Vehicle data over the EU Data Act portal: find the vehicle's data request, list its packages,
//! download the new ones and merge them (WP-25 stage 2).
//!
//! The portal produces a package roughly every fifteen minutes; each one is downloaded exactly
//! once. What the packages said is kept in the session, so a restart does not lose the last known
//! values and a quiet stretch does not turn them into gaps. On the first fetch, the latest
//! [`MAX_BACKFILL`] packages are read, as evcc does (`vehicle/vw/eudataact/store.go`).

pub(crate) mod document;
pub(crate) mod normalise;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use vag2mqtt_connector_api::ConnectorError;
use vag2mqtt_domain::Vin;

use crate::auth::HttpClient;
use crate::error::{Step, manufacturer, parsing};
use crate::trace::Trace;
use document::{Points, parse_timestamp, read_package};

const LOG: &str = "vag2mqtt::fetch";

/// How many packages the first fetch reads.
pub(crate) const MAX_BACKFILL: usize = 8;

/// The suffix the portal gives a package that carries nothing (B-01).
const NO_CONTENT_SUFFIX: &str = "_no_content_found.zip";

/// What the connector remembers per vehicle between fetches.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ExportState {
    /// The data request identifier, looked up once.
    pub(crate) identifier: Option<String>,
    /// The creation time of the newest package already read.
    pub(crate) after: Option<DateTime<Utc>>,
    /// What the packages said, field by field.
    pub(crate) points: Points,
}

/// One entry of the package list.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawDataset {
    name: Option<String>,
    created_on: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DatasetList {
    Bare(Vec<RawDataset>),
    Wrapped { files: Vec<RawDataset> },
}

/// A package worth downloading.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Dataset {
    pub(crate) name: String,
    pub(crate) created_on: DateTime<Utc>,
}

/// How many packages a fetch found and read, for the log.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct FetchReport {
    pub(crate) listed: usize,
    pub(crate) empty: usize,
    pub(crate) read: usize,
    pub(crate) unreadable: usize,
}

/// The portal endpoints for one vehicle.
pub(crate) struct VehicleEndpoints<'a> {
    pub(crate) base: &'a Url,
    pub(crate) vin: &'a Vin,
}

impl VehicleEndpoints<'_> {
    fn url(&self, path: String) -> Url {
        let mut url = self.base.clone();
        url.set_path(&path);
        url
    }

    fn metadata(&self) -> Url {
        self.url(format!(
            "/proxy_api/euda-apim/datarequest/vehicles/{}/metadata/partial",
            self.vin.as_str()
        ))
    }

    fn list(&self, identifier: &str) -> Url {
        self.url(format!(
            "/proxy_api/euda-apim/datadelivery/vehicles/{}/{identifier}/list",
            self.vin.as_str()
        ))
    }

    fn download(&self, identifier: &str) -> Url {
        self.url(format!(
            "/proxy_api/euda-apim/datadelivery/vehicles/{}/{identifier}/download",
            self.vin.as_str()
        ))
    }
}

/// Reads every package not seen yet into `state`.
pub(crate) async fn update(
    http: &HttpClient,
    endpoints: &VehicleEndpoints<'_>,
    state: &mut ExportState,
    trace: &mut Trace,
) -> Result<FetchReport, ConnectorError> {
    let identifier = match &state.identifier {
        Some(identifier) => identifier.clone(),
        None => {
            let identifier = request_identifier(http, endpoints, trace).await?;
            state.identifier = Some(identifier.clone());
            identifier
        }
    };

    let listed = list_datasets(http, endpoints, &identifier, trace).await?;
    let mut report = FetchReport {
        listed: listed.len(),
        ..FetchReport::default()
    };
    let content = content_datasets(listed, &mut report);
    for dataset in pending(content, state.after) {
        // The mark moves before the package is read, so a package that cannot be read is not
        // downloaded again on every poll.
        state.after = Some(dataset.created_on);
        let url = endpoints.download(&identifier);
        let headers = [("filename", dataset.name.as_str()), ("type", "partial")];
        let (status, bytes) = http
            .get_bytes(Step::DatasetDownload, url, &headers, trace)
            .await?;
        check_status(Step::DatasetDownload, status.as_u16())?;
        match read_package(&bytes) {
            Ok(package) => {
                if package
                    .vin
                    .as_deref()
                    .is_some_and(|vin| !vin.eq_ignore_ascii_case(endpoints.vin.as_str()))
                {
                    tracing::warn!(target: LOG, vin = %endpoints.vin.short(), "a package for another vehicle was skipped");
                    report.unreadable += 1;
                    continue;
                }
                state.points.merge(package, normalise::is_relevant);
                report.read += 1;
            }
            Err(error) => {
                tracing::warn!(target: LOG, vin = %endpoints.vin.short(), %error, "an export package could not be read");
                report.unreadable += 1;
            }
        }
    }
    trace.outcome(
        Step::DatasetDownload,
        &format!(
            "{} listed, {} empty, {} read, {} unreadable",
            report.listed, report.empty, report.read, report.unreadable
        ),
    );
    Ok(report)
}

async fn request_identifier(
    http: &HttpClient,
    endpoints: &VehicleEndpoints<'_>,
    trace: &mut Trace,
) -> Result<String, ConnectorError> {
    #[derive(Deserialize)]
    struct Metadata {
        #[serde(rename = "Identifier", alias = "identifier")]
        identifier: Option<String>,
    }
    let response = http
        .get(
            Step::DataRequest,
            endpoints.metadata(),
            &[("Accept", "application/json")],
            trace,
        )
        .await?;
    let status = response.status.as_u16();
    if status == 404 {
        return Err(no_data_request());
    }
    check_status(Step::DataRequest, status)?;
    let metadata: Metadata =
        serde_json::from_str(&response.body).map_err(|_| parsing(Step::DataRequest))?;
    let identifier = metadata
        .identifier
        .filter(|id| !id.is_empty())
        .ok_or_else(no_data_request)?;
    trace.outcome(Step::DataRequest, "data request found");
    Ok(identifier)
}

fn no_data_request() -> ConnectorError {
    ConnectorError::Manufacturer {
        status: Some(404),
        code: Some(
            "no continuous data request is configured in the portal for this vehicle".into(),
        ),
    }
}

async fn list_datasets(
    http: &HttpClient,
    endpoints: &VehicleEndpoints<'_>,
    identifier: &str,
    trace: &mut Trace,
) -> Result<Vec<Dataset>, ConnectorError> {
    let response = http
        .get(
            Step::DatasetList,
            endpoints.list(identifier),
            &[("Accept", "application/json"), ("type", "partial")],
            trace,
        )
        .await?;
    let status = response.status.as_u16();
    if status == 404 {
        return Ok(Vec::new());
    }
    check_status(Step::DatasetList, status)?;
    parse_dataset_list(&response.body)
}

fn check_status(step: Step, status: u16) -> Result<(), ConnectorError> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(ConnectorError::SessionExpired),
        429 => Err(ConnectorError::RateLimited { retry_after: None }),
        _ => {
            tracing::warn!(target: LOG, step = %step, status, "unexpected status from the portal");
            Err(manufacturer(status, None))
        }
    }
}

/// Parses the package list. An entry without a name or a readable creation time is skipped.
pub(crate) fn parse_dataset_list(body: &str) -> Result<Vec<Dataset>, ConnectorError> {
    let list: DatasetList = serde_json::from_str(body).map_err(|_| parsing(Step::DatasetList))?;
    let raw = match list {
        DatasetList::Bare(raw) => raw,
        DatasetList::Wrapped { files } => files,
    };
    Ok(raw
        .into_iter()
        .filter_map(|entry| {
            Some(Dataset {
                name: entry.name.filter(|n| !n.is_empty())?,
                created_on: entry.created_on.as_deref().and_then(parse_timestamp)?,
            })
        })
        .collect())
}

/// Drops the packages the portal marks as empty and sorts the rest oldest first.
fn content_datasets(list: Vec<Dataset>, report: &mut FetchReport) -> Vec<Dataset> {
    let mut content: Vec<Dataset> = list
        .into_iter()
        .filter(|d| {
            let empty = d.name.to_ascii_lowercase().ends_with(NO_CONTENT_SUFFIX);
            if empty {
                report.empty += 1;
            }
            !empty
        })
        .collect();
    content.sort_by_key(|d| d.created_on);
    content
}

/// The packages still to read: after the mark, or the latest few on the first fetch.
fn pending(content: Vec<Dataset>, after: Option<DateTime<Utc>>) -> Vec<Dataset> {
    match after {
        None => {
            let skip = content.len().saturating_sub(MAX_BACKFILL);
            content.into_iter().skip(skip).collect()
        }
        Some(after) => content
            .into_iter()
            .filter(|d| d.created_on > after)
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minute: u32) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(&format!("2026-09-24T10:{minute:02}:00Z"))
            .unwrap()
            .with_timezone(&Utc)
    }

    fn dataset(name: &str, minute: u32) -> Dataset {
        Dataset {
            name: name.into(),
            created_on: at(minute),
        }
    }

    #[test]
    fn both_list_shapes_parse_and_broken_entries_are_skipped() {
        let bare = r#"[{"name":"a.zip","createdOn":"2026-09-24T10:00:00Z"},{"name":"b.zip"}]"#;
        assert_eq!(parse_dataset_list(bare).unwrap(), vec![dataset("a.zip", 0)]);
        let wrapped =
            r#"{"files":[{"name":"c.zip","createdOn":"2026-09-24T10:15:00.1234Z","size":3}]}"#;
        assert_eq!(parse_dataset_list(wrapped).unwrap().len(), 1);
        assert!(parse_dataset_list("<html>").is_err());
    }

    #[test]
    fn empty_packages_are_counted_and_dropped_and_the_rest_sorted() {
        let mut report = FetchReport::default();
        let content = content_datasets(
            vec![
                dataset("x_2_no_content_found.zip", 30),
                dataset("x_2.zip", 15),
                dataset("x_1.zip", 0),
                dataset("X_3_NO_CONTENT_FOUND.ZIP", 45),
            ],
            &mut report,
        );
        assert_eq!(report.empty, 2);
        assert_eq!(content, vec![dataset("x_1.zip", 0), dataset("x_2.zip", 15)]);
    }

    #[test]
    fn the_first_fetch_backfills_and_later_ones_read_only_what_is_new() {
        let content: Vec<Dataset> = (0..12).map(|i| dataset(&format!("p{i}.zip"), i)).collect();
        let first = pending(content.clone(), None);
        assert_eq!(first.len(), MAX_BACKFILL);
        assert_eq!(first[0].name, "p4.zip");
        let later = pending(content, Some(at(10)));
        assert_eq!(later, vec![dataset("p11.zip", 11)]);
    }

    #[test]
    fn statuses_map_onto_connector_errors() {
        assert!(check_status(Step::DatasetList, 200).is_ok());
        assert!(matches!(
            check_status(Step::DatasetList, 403),
            Err(ConnectorError::SessionExpired)
        ));
        assert!(matches!(
            check_status(Step::DatasetList, 429),
            Err(ConnectorError::RateLimited { .. })
        ));
        assert!(matches!(
            check_status(Step::DatasetList, 502),
            Err(ConnectorError::Manufacturer {
                status: Some(502),
                ..
            })
        ));
    }
}
