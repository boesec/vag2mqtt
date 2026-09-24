//! The EU Data Act portal route.
//!
//! Volkswagen Group Info Services AG operates an official portal under the EU Data Act at
//! `https://eu-data-act.drivesomethinggreater.com`. It is the only route into an Audi account
//! that is open to a third party: both native routes are shut
//! (`Docs/reference/audi-auth.md` sections 1c and 1d).
//!
//! The login **is** the form login of [`super::form`]: same identity service, same scraped
//! e-mail and password forms, same redirect chase. Only three things differ:
//!
//! - the OAuth parameters (a different `client_id`, an `https` redirect URI, a smaller scope);
//! - there is no token exchange, because the portal is the confidential client and performs the
//!   exchange itself;
//! - the session is therefore a cookie, not a bearer token.
//!
//! What the route costs is recorded in `Docs/reference/audi-auth.md` section 1e: data appears
//! roughly every fifteen minutes, there are no commands at all, and a one-time browser setup
//! (consent, vehicle linked, continuous data request switched on) is the user's job. Without
//! that setup the portal answers the vehicle list with `403`, and this module says so in those
//! words rather than blaming the credentials.

use async_trait::async_trait;
use serde::Deserialize;
use url::Url;
use vag2mqtt_connector_api::{
    ConnectorError, ConnectorInfo, Credentials, DiscoveredVehicle, SessionState,
};
use vag2mqtt_domain::{Brand, DataSourceKind, Drivetrain, VehicleState, Vin};

use super::AudiAuthStrategy;
use super::form::{CODE_RESPONSE_TYPE, Endpoints, Pkce, Signin};
use super::http::HttpClient;
use crate::connector::MIN_PORTAL_POLLING_INTERVAL;
use crate::error::{Step, parsing};
use crate::session::PortalSession;
use crate::trace::Trace;

const LOG: &str = "vag2mqtt::auth";

/// The portal's base URL.
pub const PORTAL_BASE: &str = "https://eu-data-act.drivesomethinggreater.com";

/// The portal's OAuth client id for Audi (`Docs/reference/audi-auth.md` section 1e).
pub const AUDI_PORTAL_CLIENT_ID: &str = "cc29b87a-5e9a-4362-aecf-5adea6b01bbb@apps_vw-dilab_com";

/// The scope the portal asks for.
pub const PORTAL_SCOPE: &str = "openid cars profile";

/// The vehicle list endpoint, relative to the portal base.
const VEHICLES_PATH: &str = "/proxy_api/consent/me/vehicles";

/// What the connector says when the portal refuses to talk about vehicles.
///
/// A `403` here does not mean the password was wrong: the login succeeded, the account simply
/// has not completed the one-time setup in the browser. Saying "invalid credentials" would send
/// the user to change a password that is perfectly fine.
const SETUP_MISSING: &str = "portal setup incomplete: open the portal in a browser, accept the \
consent, link the vehicle and switch on a continuous data request";

/// The EU Data Act portal route.
#[derive(Clone, Debug)]
pub struct EuDataActStrategy {
    signin: Signin,
    base: Url,
}

impl EuDataActStrategy {
    /// The production portal.
    pub fn new() -> Result<Self, ConnectorError> {
        let base = Url::parse(PORTAL_BASE)
            .map_err(|_| ConnectorError::network("the portal base URL is not a URL"))?;
        let endpoints = portal_endpoints(&base);
        Ok(Self::with_endpoints(base, endpoints))
    }

    /// A portal at `base` with explicit OAuth parameters, for tests against a mock server.
    pub fn with_endpoints(base: Url, endpoints: Endpoints) -> Self {
        Self {
            signin: Signin { endpoints },
            base,
        }
    }

    /// The portal's authority, used to recognise that the redirect chase has arrived.
    fn authority(&self) -> &str {
        self.base.authority()
    }

    fn vehicles_url(&self) -> Url {
        let mut url = self.base.clone();
        url.set_path(VEHICLES_PATH);
        url.set_query(Some("viewPosition=FRONT_LEFT"));
        url
    }

    /// Asks the portal for the account's vehicles, using a client seeded with `session`.
    ///
    /// Returns the vehicles and the jar as it stands afterwards, because the portal may rotate
    /// its cookie and the caller has to store what is current.
    async fn request_vehicles(
        &self,
        session: &PortalSession,
        trace: &mut Trace,
    ) -> Result<(Vec<DiscoveredVehicle>, crate::cookies::Cookies), ConnectorError> {
        let http = HttpClient::with_cookies(session.cookies.clone())?;
        let response = http
            .get(
                Step::VehicleList,
                self.vehicles_url(),
                &[("Accept", "application/json")],
                trace,
            )
            .await?;
        let status = response.status.as_u16();
        if status == 401 || status == 403 {
            trace.outcome(
                Step::VehicleList,
                &format!("HTTP {status}, the portal does not accept this session"),
            );
            return Err(ConnectorError::SessionExpired);
        }
        if !response.status.is_success() {
            return Err(self.signin.unexpected(Step::VehicleList, &response));
        }
        let vehicles = parse_vehicles(&response.body)?;
        trace.outcome(
            Step::VehicleList,
            &format!("{} vehicle(s) on the account", vehicles.len()),
        );
        Ok((vehicles, http.cookies()))
    }
}

/// The OAuth parameters of the portal, derived from its base URL.
///
/// `bff_base` and `x_client_id` belong to the native route's token exchange and are unused here,
/// because the portal performs no exchange at all. They are filled with the portal base and an
/// empty string so that the shared [`Endpoints`] type stays one type.
pub fn portal_endpoints(base: &Url) -> Endpoints {
    let mut redirect = base.clone();
    redirect.set_path("/login");
    Endpoints {
        identity_base: Endpoints::default().identity_base,
        bff_base: base.clone(),
        client_id: AUDI_PORTAL_CLIENT_ID.into(),
        redirect_uri: redirect.to_string(),
        scope: PORTAL_SCOPE.into(),
        x_client_id: String::new(),
        response_type: CODE_RESPONSE_TYPE.into(),
    }
}

#[async_trait]
impl AudiAuthStrategy for EuDataActStrategy {
    fn name(&self) -> &'static str {
        "eu_data_act"
    }

    fn info(&self) -> ConnectorInfo {
        ConnectorInfo {
            brand: Brand::Audi,
            kind: DataSourceKind::Export,
            min_polling_interval: MIN_PORTAL_POLLING_INTERVAL,
            supports_commands: false,
        }
    }

    async fn login(
        &self,
        credentials: &Credentials,
        trace: &mut Trace,
    ) -> Result<SessionState, ConnectorError> {
        let result = self.run_login(credentials, trace).await;
        match &result {
            Ok(session) => trace.outcome(
                Step::PortalLanding,
                &format!("ok, session established at {}", session.established_at),
            ),
            Err(error) => trace.outcome(Step::PortalLanding, &format!("failed: {error}")),
        }
        trace.finish();
        result.map(PortalSession::into_session)
    }

    async fn refresh(
        &self,
        session: &mut SessionState,
        trace: &mut Trace,
    ) -> Result<(), ConnectorError> {
        // The portal issues no refresh token and states no expiry, so the only way to renew is
        // to find out whether the cookie still works. A rejection becomes `SessionExpired`, and
        // the supervisor answers that with a fresh login (WP-05).
        let stored = PortalSession::from_session(session)?;
        let (_, cookies) = self.request_vehicles(&stored, trace).await?;
        trace.finish();
        *session = PortalSession {
            cookies,
            established_at: stored.established_at,
        }
        .into_session();
        Ok(())
    }

    /// Stage 2 of WP-25, and it waits on B-01.
    ///
    /// Normalising an archive that nobody has seen would be guesswork, and inventing default
    /// values is exactly what DR-002 forbids. So the route says what it does not have.
    async fn fetch_state(
        &self,
        _session: &mut SessionState,
        _vin: &Vin,
        _trace: &mut Trace,
    ) -> Result<VehicleState, ConnectorError> {
        Err(ConnectorError::Unsupported {
            operation: "fetch_state over the EU Data Act portal: WP-25 stage 2, blocked by B-01 \
(the portal currently delivers empty data packages)",
        })
    }

    async fn list_vehicles(
        &self,
        session: &mut SessionState,
        trace: &mut Trace,
    ) -> Result<Vec<DiscoveredVehicle>, ConnectorError> {
        let stored = PortalSession::from_session(session)?;
        let (vehicles, cookies) = self.request_vehicles(&stored, trace).await?;
        trace.finish();
        *session = PortalSession {
            cookies,
            established_at: stored.established_at,
        }
        .into_session();
        Ok(vehicles)
    }
}

impl EuDataActStrategy {
    async fn run_login(
        &self,
        credentials: &Credentials,
        trace: &mut Trace,
    ) -> Result<PortalSession, ConnectorError> {
        let http = HttpClient::new()?;

        // The portal hands out a first cookie on its landing page, and the sign-in the
        // authorize request starts is bound to it.
        let _ = http
            .get(Step::Authorize, self.base.clone(), &[], trace)
            .await?;

        let pkce = Pkce::generate();
        let posted = self
            .signin
            .sign_in(&http, credentials, &pkce, trace)
            .await?;

        // There is no token exchange. The chase runs through the redirect URI rather than
        // stopping at it, because that request is the one the portal answers with its session.
        let landed = self
            .signin
            .chase_into_portal(&http, self.authority(), posted, trace)
            .await?;
        tracing::debug!(
            target: LOG,
            path = landed.url.path(),
            "landed on the portal"
        );

        let cookies = http.cookies();
        if cookies.is_empty() {
            return Err(parsing(Step::PortalLanding));
        }
        let session = PortalSession::new(cookies);

        // Prove the session before storing it. A portal that answers the vehicle list is a
        // portal we are logged in to; one that refuses is worth a clear message now rather than
        // an unexplained failure on the first poll.
        match self.request_vehicles(&session, trace).await {
            Ok((vehicles, cookies)) => {
                tracing::info!(
                    target: LOG,
                    vehicles = vehicles.len(),
                    "portal session established"
                );
                Ok(PortalSession {
                    cookies,
                    established_at: session.established_at,
                })
            }
            Err(ConnectorError::SessionExpired) => {
                // The login itself worked; the account has not finished the browser setup.
                tracing::warn!(target: LOG, "{SETUP_MISSING}");
                Err(ConnectorError::Manufacturer {
                    status: Some(403),
                    code: Some(SETUP_MISSING.to_string()),
                })
            }
            Err(other) => Err(other),
        }
    }
}

/// One entry of the portal's vehicle list.
///
/// Every field is optional: the portal's shapes are not documented and a missing field must
/// never cost us the whole list.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PortalVehicle {
    vin: Option<String>,
    vehicle_identification_number: Option<String>,
    model_name: Option<String>,
    model: Option<String>,
    fuel_type: Option<String>,
    engine_type: Option<String>,
}

/// The envelopes the portal might wrap its list in.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum VehicleList {
    Bare(Vec<PortalVehicle>),
    Wrapped {
        #[serde(alias = "data", alias = "content", alias = "items")]
        vehicles: Vec<PortalVehicle>,
    },
}

/// Parses the vehicle list, skipping entries without a usable VIN.
///
/// An entry the portal lists but cannot name is not an error for the other entries: isolation
/// applies inside one response too.
fn parse_vehicles(body: &str) -> Result<Vec<DiscoveredVehicle>, ConnectorError> {
    let list: VehicleList = serde_json::from_str(body).map_err(|_| parsing(Step::VehicleList))?;
    let entries = match list {
        VehicleList::Bare(entries) => entries,
        VehicleList::Wrapped { vehicles } => vehicles,
    };
    let mut discovered = Vec::with_capacity(entries.len());
    for entry in entries {
        let raw = entry
            .vin
            .as_deref()
            .or(entry.vehicle_identification_number.as_deref());
        let Some(vin) = raw.and_then(|vin| Vin::new(vin).ok()) else {
            tracing::warn!(
                target: LOG,
                "the portal listed a vehicle without a usable VIN; skipping it"
            );
            continue;
        };
        discovered.push(DiscoveredVehicle {
            vin,
            model: entry.model_name.or(entry.model),
            drivetrain: drivetrain_of(entry.fuel_type.as_deref(), entry.engine_type.as_deref()),
        });
    }
    Ok(discovered)
}

/// Maps the portal's propulsion words onto the domain's drivetrain.
///
/// Anything unrecognised stays [`Drivetrain::Unknown`] rather than being guessed, because the
/// drivetrain decides which categories a consumer expects to exist.
fn drivetrain_of(fuel_type: Option<&str>, engine_type: Option<&str>) -> Drivetrain {
    let text = [fuel_type, engine_type]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if text.contains("hybrid") || text.contains("phev") {
        return Drivetrain::Hybrid;
    }
    if text.contains("electric") || text.contains("bev") || text.contains("elektro") {
        return Drivetrain::Electric;
    }
    if ["petrol", "gasoline", "diesel", "benzin", "cng", "lpg"]
        .iter()
        .any(|word| text.contains(word))
    {
        return Drivetrain::Combustion;
    }
    Drivetrain::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_production_parameters_are_the_documented_ones() {
        let strategy = EuDataActStrategy::new().unwrap();
        assert_eq!(strategy.name(), "eu_data_act");
        assert_eq!(
            strategy.authority(),
            "eu-data-act.drivesomethinggreater.com"
        );
        let info = strategy.info();
        assert_eq!(info.kind, DataSourceKind::Export);
        assert!(!info.supports_commands, "the portal is read only");
        assert_eq!(info.min_polling_interval.as_secs(), 900);
    }

    #[test]
    fn the_redirect_uri_is_the_portal_login_page() {
        let base = Url::parse(PORTAL_BASE).unwrap();
        let endpoints = portal_endpoints(&base);
        assert_eq!(
            endpoints.redirect_uri,
            "https://eu-data-act.drivesomethinggreater.com/login"
        );
        assert_eq!(endpoints.client_id, AUDI_PORTAL_CLIENT_ID);
        assert_eq!(endpoints.scope, "openid cars profile");
        assert_eq!(
            endpoints.identity_base.as_str(),
            "https://identity.vwgroup.io/"
        );
    }

    #[test]
    fn a_bare_list_parses() {
        let body = r#"[
            {"vin":"WAUZZZ0000000TEST","modelName":"A6 e-tron","fuelType":"ELECTRIC"}
        ]"#;
        let vehicles = parse_vehicles(body).unwrap();
        assert_eq!(vehicles.len(), 1);
        assert_eq!(vehicles[0].vin.as_str(), "WAUZZZ0000000TEST");
        assert_eq!(vehicles[0].model.as_deref(), Some("A6 e-tron"));
        assert_eq!(vehicles[0].drivetrain, Drivetrain::Electric);
    }

    #[test]
    fn a_wrapped_list_parses_and_unknown_fields_are_tolerated() {
        let body = r#"{"vehicles":[
            {"vehicleIdentificationNumber":"WAUZZZ0000000TEST","surprise":42}
        ]}"#;
        let vehicles = parse_vehicles(body).unwrap();
        assert_eq!(vehicles.len(), 1);
        assert_eq!(vehicles[0].model, None);
        assert_eq!(
            vehicles[0].drivetrain,
            Drivetrain::Unknown,
            "an undeclared propulsion is not guessed"
        );
    }

    #[test]
    fn an_entry_without_a_usable_vin_is_skipped_rather_than_fatal() {
        let body = r#"[
            {"vin":"not-a-vin"},
            {"modelName":"nameless"},
            {"vin":"WAUZZZ0000000TEST"}
        ]"#;
        let vehicles = parse_vehicles(body).unwrap();
        assert_eq!(vehicles.len(), 1);
        assert_eq!(vehicles[0].vin.as_str(), "WAUZZZ0000000TEST");
    }

    #[test]
    fn a_body_that_is_not_a_list_is_a_parsing_error() {
        assert!(matches!(
            parse_vehicles("<html>login</html>"),
            Err(ConnectorError::Parsing {
                context: "vehicle_list"
            })
        ));
    }

    #[test]
    fn propulsion_words_map_without_guessing() {
        assert_eq!(drivetrain_of(Some("Diesel"), None), Drivetrain::Combustion);
        assert_eq!(
            drivetrain_of(None, Some("PLUGIN_HYBRID")),
            Drivetrain::Hybrid
        );
        assert_eq!(drivetrain_of(Some("BEV"), None), Drivetrain::Electric);
        assert_eq!(drivetrain_of(Some("mystery"), None), Drivetrain::Unknown);
        assert_eq!(drivetrain_of(None, None), Drivetrain::Unknown);
    }

    #[test]
    fn the_setup_message_blames_the_setup_and_not_the_password() {
        assert!(SETUP_MISSING.contains("consent"));
        assert!(!SETUP_MISSING.to_ascii_lowercase().contains("password"));
    }
}
