//! The web interface.
//!
//! Server rendered HTML, one hand written stylesheet, and htmx for exactly two things: polling
//! the overview fragment and swapping page loads. Every action is a plain form that works with
//! JavaScript switched off, because the server answers a post with a redirect.
//!
//! The interface talks to the runtime through the same handle the API uses, never over HTTP to
//! itself.

mod views;

use askama::Template;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use serde::Deserialize;
use vag2mqtt_domain::{AccountId, Brand, PollingConfig, Secret, Vin};
use vag2mqtt_runtime::{AccountCreate, AccountUpdate, RuntimeStatus};

use crate::state::{AppState, COMMAND_TIMEOUT};
use views::*;

const STYLE: &str = include_str!("../../assets/style.css");
const HTMX: &str = include_str!("../../assets/htmx.min.js");

const LOG: &str = "vag2mqtt::admin";

// ----- templates ------------------------------------------------------------------------

#[derive(Template)]
#[template(path = "overview.html")]
struct OverviewTemplate {
    chrome: Chrome,
    fragment: OverviewFragment,
}

#[derive(Template)]
#[template(path = "overview_fragment.html")]
struct OverviewFragmentTemplate {
    fragment: OverviewFragment,
}

#[derive(Template)]
#[template(path = "mqtt.html")]
struct MqttTemplate {
    chrome: Chrome,
    host: String,
    port: u16,
    username: String,
    configured: bool,
    tls: bool,
    topic_prefix: String,
    client_id: String,
    keep_alive_secs: u64,
    enabled: bool,
    connected: bool,
}

impl From<MqttView> for MqttTemplate {
    fn from(view: MqttView) -> Self {
        Self {
            chrome: view.chrome,
            host: view.host,
            port: view.port,
            username: view.username,
            configured: view.configured,
            tls: view.tls,
            topic_prefix: view.topic_prefix,
            client_id: view.client_id,
            keep_alive_secs: view.keep_alive_secs,
            enabled: view.enabled,
            connected: view.connected,
        }
    }
}

#[derive(Template)]
#[template(path = "account_new.html")]
struct AccountNewTemplate {
    chrome: Chrome,
    brands: Vec<BrandOption>,
    username: String,
    polling_interval_secs: u64,
}

#[derive(Template)]
#[template(path = "account.html")]
struct AccountTemplate {
    chrome: Chrome,
    account: AccountRow,
    polling_interval_secs: u64,
    has_error: bool,
    error_detail: ErrorDetail,
    respawns: u32,
    consecutive_failures: u32,
    running: bool,
    vehicles: Vec<VehicleRow>,
}

#[derive(Template)]
#[template(path = "vehicle.html")]
struct VehicleTemplate {
    chrome: Chrome,
    vehicle: VehicleRow,
    account_id: String,
    model: String,
    display_name: String,
    drivetrain: String,
    missing_since: String,
    has_error: bool,
    error_detail: ErrorDetail,
    highlights: Vec<DiagnosticRow>,
    has_state: bool,
}

#[derive(Template)]
#[template(path = "diagnostics.html")]
struct DiagnosticsTemplate {
    chrome: Chrome,
    vin: String,
    label: String,
    fetched_at: String,
    rows: Vec<DiagnosticRow>,
    present: usize,
    unavailable: usize,
    unsupported: usize,
    has_state: bool,
    has_raw: bool,
    raw: String,
}

/// Renders a template, or answers 500 without leaking the cause.
fn page<T: Template>(template: T) -> Response {
    match template.render() {
        Ok(html) => Html(html).into_response(),
        Err(error) => {
            tracing::error!(target: LOG, %error, "a template failed to render");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("<h1>Something went wrong</h1><p>The page could not be rendered.</p>"),
            )
                .into_response()
        }
    }
}

// ----- routes ---------------------------------------------------------------------------

/// The interface's routes, merged into the same router as the API.
pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(overview))
        .route("/fragments/overview", get(overview_fragment))
        .route("/assets/style.css", get(style))
        .route("/assets/htmx.min.js", get(htmx))
        .route("/mqtt", get(mqtt_form).post(mqtt_save))
        .route("/accounts/new", get(account_new))
        .route("/accounts", post(account_create))
        .route("/accounts/{id}", get(account_page).post(account_save))
        .route("/accounts/{id}/credentials", post(account_credentials))
        .route("/accounts/{id}/enabled", post(account_enabled))
        .route(
            "/accounts/{id}/reauthenticate",
            post(account_reauthenticate),
        )
        .route("/accounts/{id}/delete", post(account_delete))
        .route("/vehicles/{vin}", get(vehicle_page).post(vehicle_save))
        .route("/vehicles/{vin}/enabled", post(vehicle_enabled))
        .route("/vehicles/{vin}/refresh", post(vehicle_refresh))
        .route("/vehicles/{vin}/delete", post(vehicle_delete))
        .route("/vehicles/{vin}/diagnostics", get(diagnostics))
}

// ----- assets ---------------------------------------------------------------------------

async fn style() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], STYLE)
}

async fn htmx() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        HTMX,
    )
}

// ----- overview -------------------------------------------------------------------------

async fn overview(State(state): State<AppState>) -> Response {
    page(OverviewTemplate {
        chrome: Chrome::new("overview"),
        fragment: OverviewFragment::new(state.status()),
    })
}

async fn overview_fragment(State(state): State<AppState>) -> Response {
    page(OverviewFragmentTemplate {
        fragment: OverviewFragment::new(state.status()),
    })
}

// ----- broker ---------------------------------------------------------------------------

async fn mqtt_form(State(state): State<AppState>) -> Response {
    let status = state.status();
    page(MqttTemplate::from(MqttView::new(
        Chrome::new("mqtt"),
        &status,
    )))
}

/// The broker form. Checkboxes are absent when unticked, hence the `Option<String>`.
#[derive(Deserialize)]
struct MqttForm {
    host: String,
    port: Option<u16>,
    username: Option<String>,
    password: Option<String>,
    tls: Option<String>,
    topic_prefix: Option<String>,
    client_id: Option<String>,
    keep_alive_secs: Option<u64>,
    enabled: Option<String>,
}

async fn mqtt_save(State(state): State<AppState>, Form(form): Form<MqttForm>) -> Response {
    let request = crate::dto::ConfigureMqttRequest {
        host: form.host,
        port: form.port,
        username: form.username.filter(|value| !value.trim().is_empty()),
        password: form
            .password
            .filter(|value| !value.is_empty())
            .map(Secret::new),
        tls: Some(form.tls.is_some()),
        topic_prefix: form.topic_prefix.filter(|value| !value.trim().is_empty()),
        client_id: form.client_id.filter(|value| !value.trim().is_empty()),
        keep_alive_secs: form.keep_alive_secs,
        // The form offers no protocol switch: MQTT 5 is accepted by the API but served with
        // 3.1.1 until WP-24, so a control for it would promise something we do not do.
        protocol: None,
        enabled: Some(form.enabled.is_some()),
    };
    let outcome = match request.into_config() {
        Ok((config, password)) => confirm(state.runtime().configure_mqtt(config, password)).await,
        Err(error) => Err(message_of(error)),
    };
    match outcome {
        Ok(()) => Redirect::to("/").into_response(),
        Err(message) => {
            let status = state.status();
            page(MqttTemplate::from(MqttView::new(
                Chrome::with_error("mqtt", message),
                &status,
            )))
        }
    }
}

// ----- accounts -------------------------------------------------------------------------

async fn account_new(State(state): State<AppState>) -> Response {
    page(AccountNewTemplate {
        chrome: Chrome::new("account_new"),
        brands: BrandOption::list(state.brands()),
        username: String::new(),
        polling_interval_secs: 900,
    })
}

/// The new account form.
#[derive(Deserialize)]
struct AccountCreateForm {
    brand: String,
    username: String,
    password: String,
    polling_interval_secs: Option<u64>,
}

async fn account_create(
    State(state): State<AppState>,
    Form(form): Form<AccountCreateForm>,
) -> Response {
    let retry = |message: String, username: String, interval: u64| {
        page(AccountNewTemplate {
            chrome: Chrome::with_error("account_new", message),
            brands: BrandOption::list(state.brands()),
            username,
            polling_interval_secs: interval,
        })
    };
    let interval = form.polling_interval_secs.unwrap_or(900);
    let Some(brand) = parse_brand(&form.brand) else {
        return retry(
            format!("`{}` is not a brand this build supports", form.brand),
            form.username,
            interval,
        );
    };
    let input = AccountCreate {
        brand,
        username: form.username.clone(),
        password: Secret::new(form.password),
        polling: form.polling_interval_secs.map(|secs| PollingConfig {
            interval: std::time::Duration::from_secs(secs),
        }),
        enabled: true,
    };
    match confirm(state.runtime().create_account(input)).await {
        Ok(id) => Redirect::to(&format!("/accounts/{id}")).into_response(),
        Err(message) => retry(message, form.username, interval),
    }
}

async fn account_page(State(state): State<AppState>, Path(raw): Path<String>) -> Response {
    let status = state.status();
    render_account(&state, &status, &raw, Chrome::new("overview"))
}

fn render_account(
    _state: &AppState,
    status: &RuntimeStatus,
    raw_id: &str,
    chrome: Chrome,
) -> Response {
    let Ok(id) = AccountId::new(raw_id) else {
        return not_found("No such account.");
    };
    let Some(account) = status.accounts.iter().find(|a| a.account.id == id) else {
        return not_found("No such account.");
    };
    let vehicles: Vec<VehicleRow> = status
        .vehicles
        .iter()
        .filter(|vehicle| vehicle.vehicle.account_id == id)
        .map(|vehicle| VehicleRow::new(vehicle, status.snapshot_at))
        .collect();
    page(AccountTemplate {
        chrome,
        account: AccountRow::new(account, status.snapshot_at),
        polling_interval_secs: account.account.polling.interval.as_secs(),
        has_error: account.account.last_error.is_some(),
        error_detail: ErrorDetail::new(account.account.last_error.as_ref()),
        respawns: account.respawns,
        consecutive_failures: account.consecutive_failures,
        running: account.running,
        vehicles,
    })
}

/// The account settings form.
#[derive(Deserialize)]
struct AccountSaveForm {
    username: String,
    polling_interval_secs: u64,
}

async fn account_save(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    Form(form): Form<AccountSaveForm>,
) -> Response {
    let Ok(id) = AccountId::new(&raw) else {
        return not_found("No such account.");
    };
    let update = AccountUpdate {
        username: Some(form.username),
        polling: Some(PollingConfig {
            interval: std::time::Duration::from_secs(form.polling_interval_secs),
        }),
    };
    let outcome = confirm(state.runtime().update_account(&id, update)).await;
    after(state, &raw, outcome).await
}

/// The password form.
#[derive(Deserialize)]
struct PasswordForm {
    password: String,
}

async fn account_credentials(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    Form(form): Form<PasswordForm>,
) -> Response {
    let Ok(id) = AccountId::new(&raw) else {
        return not_found("No such account.");
    };
    let outcome = confirm(
        state
            .runtime()
            .update_credentials(&id, Secret::new(form.password)),
    )
    .await;
    after(state, &raw, outcome).await
}

/// A toggle form.
#[derive(Deserialize)]
struct EnabledForm {
    enabled: String,
}

impl EnabledForm {
    fn value(&self) -> bool {
        self.enabled == "true"
    }
}

async fn account_enabled(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    Form(form): Form<EnabledForm>,
) -> Response {
    let Ok(id) = AccountId::new(&raw) else {
        return not_found("No such account.");
    };
    let outcome = confirm(state.runtime().set_account_enabled(&id, form.value())).await;
    after(state, &raw, outcome).await
}

async fn account_reauthenticate(
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> Response {
    let Ok(id) = AccountId::new(&raw) else {
        return not_found("No such account.");
    };
    let outcome = confirm(state.runtime().reauthenticate(&id)).await;
    after(state, &raw, outcome).await
}

async fn account_delete(State(state): State<AppState>, Path(raw): Path<String>) -> Response {
    let Ok(id) = AccountId::new(&raw) else {
        return not_found("No such account.");
    };
    match confirm(state.runtime().delete_account(&id)).await {
        Ok(()) => Redirect::to("/").into_response(),
        Err(message) => {
            let status = state.status();
            render_account(
                &state,
                &status,
                &raw,
                Chrome::with_error("overview", message),
            )
        }
    }
}

/// After a write: back to the account page, carrying an error when there was one.
async fn after(state: AppState, raw_id: &str, outcome: Result<(), String>) -> Response {
    match outcome {
        Ok(()) => Redirect::to(&format!("/accounts/{raw_id}")).into_response(),
        Err(message) => {
            let status = state.status();
            render_account(
                &state,
                &status,
                raw_id,
                Chrome::with_error("overview", message),
            )
        }
    }
}

// ----- vehicles -------------------------------------------------------------------------

async fn vehicle_page(State(state): State<AppState>, Path(raw): Path<String>) -> Response {
    let status = state.status();
    render_vehicle(&status, &raw, Chrome::new("overview"))
}

fn render_vehicle(status: &RuntimeStatus, raw_vin: &str, chrome: Chrome) -> Response {
    let Ok(vin) = Vin::new(raw_vin) else {
        return not_found("No such vehicle.");
    };
    let Some(vehicle) = status.vehicles.iter().find(|v| v.vehicle.vin == vin) else {
        return not_found("No such vehicle.");
    };
    page(VehicleTemplate {
        chrome,
        vehicle: VehicleRow::new(vehicle, status.snapshot_at),
        account_id: vehicle.vehicle.account_id.to_string(),
        model: vehicle.vehicle.model.clone().unwrap_or_else(|| "—".into()),
        display_name: vehicle.vehicle.display_name.clone().unwrap_or_default(),
        drivetrain: drivetrain_name(vehicle.vehicle.drivetrain),
        missing_since: moment(vehicle.vehicle.missing_since),
        has_error: vehicle.vehicle.last_error.is_some(),
        error_detail: ErrorDetail::new(vehicle.vehicle.last_error.as_ref()),
        highlights: vehicle
            .last_state
            .as_ref()
            .map(highlight_rows)
            .unwrap_or_default(),
        has_state: vehicle.last_state.is_some(),
    })
}

/// The vehicle settings form.
#[derive(Deserialize)]
struct VehicleSaveForm {
    display_name: String,
}

async fn vehicle_save(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    Form(form): Form<VehicleSaveForm>,
) -> Response {
    let Ok(vin) = Vin::new(&raw) else {
        return not_found("No such vehicle.");
    };
    let name = Some(form.display_name.trim().to_string()).filter(|name| !name.is_empty());
    let outcome = confirm(state.runtime().set_vehicle_display_name(&vin, name)).await;
    after_vehicle(state, &raw, outcome).await
}

async fn vehicle_enabled(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    Form(form): Form<EnabledForm>,
) -> Response {
    let Ok(vin) = Vin::new(&raw) else {
        return not_found("No such vehicle.");
    };
    let outcome = confirm(state.runtime().set_vehicle_enabled(&vin, form.value())).await;
    after_vehicle(state, &raw, outcome).await
}

async fn vehicle_refresh(State(state): State<AppState>, Path(raw): Path<String>) -> Response {
    let Ok(vin) = Vin::new(&raw) else {
        return not_found("No such vehicle.");
    };
    let outcome = confirm(state.runtime().refresh_vehicle_now(&vin)).await;
    after_vehicle(state, &raw, outcome).await
}

async fn vehicle_delete(State(state): State<AppState>, Path(raw): Path<String>) -> Response {
    let Ok(vin) = Vin::new(&raw) else {
        return not_found("No such vehicle.");
    };
    match confirm(state.runtime().delete_vehicle(&vin)).await {
        Ok(()) => Redirect::to("/").into_response(),
        Err(message) => {
            let status = state.status();
            render_vehicle(&status, &raw, Chrome::with_error("overview", message))
        }
    }
}

async fn after_vehicle(state: AppState, raw_vin: &str, outcome: Result<(), String>) -> Response {
    match outcome {
        Ok(()) => Redirect::to(&format!("/vehicles/{raw_vin}")).into_response(),
        Err(message) => {
            let status = state.status();
            render_vehicle(&status, raw_vin, Chrome::with_error("overview", message))
        }
    }
}

async fn diagnostics(State(state): State<AppState>, Path(raw): Path<String>) -> Response {
    let status = state.status();
    let Ok(vin) = Vin::new(&raw) else {
        return not_found("No such vehicle.");
    };
    let Some(vehicle) = status.vehicles.iter().find(|v| v.vehicle.vin == vin) else {
        return not_found("No such vehicle.");
    };
    let rows = vehicle
        .last_state
        .as_ref()
        .map(diagnostic_rows)
        .unwrap_or_default();
    let count = |state: &str| rows.iter().filter(|row| row.state == state).count();
    page(DiagnosticsTemplate {
        chrome: Chrome::new("overview"),
        vin: vehicle.vehicle.vin.to_string(),
        label: vehicle.vehicle.label().to_string(),
        fetched_at: moment(vehicle.last_state.as_ref().map(|state| state.fetched_at)),
        present: count("present"),
        unavailable: count("unavailable"),
        unsupported: count("unsupported"),
        rows,
        has_state: vehicle.last_state.is_some(),
        // No connector stores raw responses yet: WP-07 is superseded and WP-25 is not started.
        has_raw: false,
        raw: String::new(),
    })
}

// ----- helpers --------------------------------------------------------------------------

/// Runs a runtime command with the same timeout the API uses, and turns a failure into a
/// sentence a person can act on.
async fn confirm<T>(
    call: impl Future<Output = Result<T, vag2mqtt_runtime::RuntimeError>>,
) -> Result<T, String> {
    match tokio::time::timeout(COMMAND_TIMEOUT, call).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(message_of(crate::error::ApiError::from(error))),
        Err(_) => Err("The runtime did not answer in time. Try again in a moment.".to_string()),
    }
}

/// The sentence an error becomes on a page.
fn message_of(error: crate::error::ApiError) -> String {
    error.user_message()
}

fn not_found(message: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(format!(
            "<!doctype html><meta charset=utf-8><title>Not found</title>\
             <link rel=stylesheet href=/assets/style.css>\
             <main><h1>Not found</h1><p>{message}</p><p><a href=\"/\">Back to the overview</a></p></main>"
        )),
    )
        .into_response()
}

fn parse_brand(raw: &str) -> Option<Brand> {
    Brand::ALL
        .iter()
        .copied()
        .find(|brand| brand.as_str() == raw)
}

fn drivetrain_name(drivetrain: vag2mqtt_domain::Drivetrain) -> String {
    match drivetrain {
        vag2mqtt_domain::Drivetrain::Electric => "electric",
        vag2mqtt_domain::Drivetrain::Combustion => "combustion",
        vag2mqtt_domain::Drivetrain::Hybrid => "hybrid",
        vag2mqtt_domain::Drivetrain::Unknown => "unknown",
    }
    .to_string()
}
