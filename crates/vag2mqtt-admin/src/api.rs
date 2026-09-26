//! The router and its handlers.
//!
//! [`ROUTES`] is the **only** place a route is registered. The router is built from it, the
//! OpenAPI document is checked against it, and a test calls every entry, so a route cannot exist
//! in the code without being described.

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{MethodRouter, get, post, put};
use axum::{Json, Router};
use tower_http::trace::TraceLayer;
use vag2mqtt_domain::{AccountId, PollingConfig, Vin};
use vag2mqtt_runtime::{AccountCreate, AccountUpdate, RuntimeHandle};

use crate::dto::*;
use crate::error::ApiError;
use crate::state::{AppState, COMMAND_TIMEOUT};

/// The OpenAPI document, embedded so the binary needs no files beside it.
const OPENAPI: &str = include_str!("../openapi.yaml");

/// One registered route.
pub struct RouteSpec {
    /// The path as axum and OpenAPI both spell it, for example `/api/accounts/{id}`.
    pub path: &'static str,
    /// The HTTP methods this path answers, upper case.
    pub methods: &'static [&'static str],
    /// Builds the handler.
    pub handler: fn() -> MethodRouter<AppState>,
}

/// Every route the API serves. The single source of truth.
pub static ROUTES: &[RouteSpec] = &[
    RouteSpec {
        path: "/api/health",
        methods: &["GET"],
        handler: || get(health),
    },
    RouteSpec {
        path: "/api/openapi.yaml",
        methods: &["GET"],
        handler: || get(openapi),
    },
    RouteSpec {
        path: "/api/status",
        methods: &["GET"],
        handler: || get(status),
    },
    RouteSpec {
        path: "/api/brands",
        methods: &["GET"],
        handler: || get(brands),
    },
    RouteSpec {
        path: "/api/mqtt",
        methods: &["GET", "PUT"],
        handler: || get(mqtt_get).put(mqtt_put),
    },
    RouteSpec {
        path: "/api/mqtt/enabled",
        methods: &["POST"],
        handler: || post(mqtt_enabled),
    },
    RouteSpec {
        path: "/api/accounts",
        methods: &["GET", "POST"],
        handler: || get(accounts_list).post(accounts_create),
    },
    RouteSpec {
        path: "/api/accounts/{id}",
        methods: &["GET", "PATCH", "DELETE"],
        handler: || get(account_get).patch(account_patch).delete(account_delete),
    },
    RouteSpec {
        path: "/api/accounts/{id}/credentials",
        methods: &["PUT"],
        handler: || put(account_credentials),
    },
    RouteSpec {
        path: "/api/accounts/{id}/enabled",
        methods: &["POST"],
        handler: || post(account_enabled),
    },
    RouteSpec {
        path: "/api/accounts/{id}/reauthenticate",
        methods: &["POST"],
        handler: || post(account_reauthenticate),
    },
    RouteSpec {
        path: "/api/vehicles",
        methods: &["GET"],
        handler: || get(vehicles_list),
    },
    RouteSpec {
        path: "/api/vehicles/{vin}",
        methods: &["GET", "PATCH", "DELETE"],
        handler: || get(vehicle_get).patch(vehicle_patch).delete(vehicle_delete),
    },
    RouteSpec {
        path: "/api/vehicles/{vin}/enabled",
        methods: &["POST"],
        handler: || post(vehicle_enabled),
    },
    RouteSpec {
        path: "/api/vehicles/{vin}/refresh",
        methods: &["POST"],
        handler: || post(vehicle_refresh),
    },
    RouteSpec {
        path: "/api/vehicles/{vin}/state",
        methods: &["GET"],
        handler: || get(vehicle_state),
    },
    RouteSpec {
        path: "/api/vehicles/{vin}/raw",
        methods: &["GET"],
        handler: || get(vehicle_raw),
    },
];

/// Builds the router from [`ROUTES`].
pub fn router(state: AppState) -> Router {
    let mut router = Router::new();
    for route in ROUTES {
        router = router.route(route.path, (route.handler)());
    }
    router.layer(TraceLayer::new_for_http()).with_state(state)
}

type ApiResult<T> = Result<Json<T>, ApiError>;

/// Runs a runtime command with the confirmation timeout.
async fn confirm<T>(
    call: impl Future<Output = Result<T, vag2mqtt_runtime::RuntimeError>>,
) -> Result<T, ApiError> {
    match tokio::time::timeout(COMMAND_TIMEOUT, call).await {
        Ok(result) => result.map_err(ApiError::from),
        Err(_) => {
            tracing::warn!(
                target: "vag2mqtt::admin",
                timeout_secs = COMMAND_TIMEOUT.as_secs(),
                "the runtime did not confirm in time"
            );
            Err(ApiError::Unavailable)
        }
    }
}

fn account_id(raw: &str) -> Result<AccountId, ApiError> {
    AccountId::new(raw).map_err(|_| ApiError::NotFound("account"))
}

fn vin(raw: &str) -> Result<Vin, ApiError> {
    Vin::new(raw).map_err(|_| ApiError::NotFound("vehicle"))
}

fn runtime(state: &AppState) -> &RuntimeHandle {
    state.runtime()
}

// ----- handlers -------------------------------------------------------------------------

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".into(),
        version: env!("CARGO_PKG_VERSION").into(),
    })
}

async fn openapi() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/yaml; charset=utf-8")],
        OPENAPI,
    )
}

async fn status(State(state): State<AppState>) -> Json<StatusResponse> {
    Json(StatusResponse::from(state.status()))
}

async fn brands(State(state): State<AppState>) -> Json<BrandsResponse> {
    Json(BrandsResponse {
        brands: state.brands().to_vec(),
    })
}

async fn mqtt_get(State(state): State<AppState>) -> Json<MqttResponse> {
    Json(MqttResponse::from(state.status().mqtt))
}

async fn mqtt_put(
    State(state): State<AppState>,
    body: Result<Json<ConfigureMqttRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<AcceptedResponse> {
    let Json(request) = body?;
    let (config, password) = request.into_config()?;
    confirm(runtime(&state).configure_mqtt(config, password)).await?;
    Ok(Json(AcceptedResponse::ok()))
}

async fn mqtt_enabled(
    State(state): State<AppState>,
    body: Result<Json<EnabledRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<AcceptedResponse> {
    let Json(request) = body?;
    confirm(runtime(&state).set_mqtt_enabled(request.enabled)).await?;
    Ok(Json(AcceptedResponse::ok()))
}

async fn accounts_list(State(state): State<AppState>) -> Json<Vec<AccountResponse>> {
    Json(
        state
            .status()
            .accounts
            .into_iter()
            .map(AccountResponse::from)
            .collect(),
    )
}

async fn accounts_create(
    State(state): State<AppState>,
    body: Result<Json<CreateAccountRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<(StatusCode, Json<CreatedAccountResponse>), ApiError> {
    let Json(request) = body?;
    let polling = request.polling_interval_secs.map(|secs| PollingConfig {
        interval: std::time::Duration::from_secs(secs),
    });
    let id = confirm(runtime(&state).create_account(AccountCreate {
        brand: request.brand,
        username: request.username,
        password: request.password,
        polling,
        enabled: request.enabled.unwrap_or(true),
    }))
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(CreatedAccountResponse { id: id.to_string() }),
    ))
}

async fn account_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<AccountResponse> {
    let id = account_id(&id)?;
    state
        .status()
        .accounts
        .into_iter()
        .find(|account| account.account.id == id)
        .map(|account| Json(AccountResponse::from(account)))
        .ok_or(ApiError::NotFound("account"))
}

async fn account_patch(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<UpdateAccountRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<AcceptedResponse> {
    let id = account_id(&id)?;
    let Json(request) = body?;
    let update = AccountUpdate {
        username: request.username,
        polling: request.polling_interval_secs.map(|secs| PollingConfig {
            interval: std::time::Duration::from_secs(secs),
        }),
    };
    confirm(runtime(&state).update_account(&id, update)).await?;
    Ok(Json(AcceptedResponse::ok()))
}

async fn account_credentials(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<UpdateCredentialsRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<AcceptedResponse> {
    let id = account_id(&id)?;
    let Json(request) = body?;
    confirm(runtime(&state).update_credentials(&id, request.password)).await?;
    Ok(Json(AcceptedResponse::ok()))
}

async fn account_enabled(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Result<Json<EnabledRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<AcceptedResponse> {
    let id = account_id(&id)?;
    let Json(request) = body?;
    confirm(runtime(&state).set_account_enabled(&id, request.enabled)).await?;
    Ok(Json(AcceptedResponse::ok()))
}

async fn account_reauthenticate(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<AcceptedResponse> {
    let id = account_id(&id)?;
    confirm(runtime(&state).reauthenticate(&id)).await?;
    Ok(Json(AcceptedResponse::ok()))
}

async fn account_delete(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<AcceptedResponse> {
    let id = account_id(&id)?;
    confirm(runtime(&state).delete_account(&id)).await?;
    Ok(Json(AcceptedResponse::ok()))
}

async fn vehicles_list(State(state): State<AppState>) -> Json<Vec<VehicleResponse>> {
    Json(
        state
            .status()
            .vehicles
            .into_iter()
            .map(VehicleResponse::from)
            .collect(),
    )
}

async fn vehicle_get(
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> ApiResult<VehicleResponse> {
    let vin = vin(&raw)?;
    state
        .status()
        .vehicles
        .into_iter()
        .find(|vehicle| vehicle.vehicle.vin == vin)
        .map(|vehicle| Json(VehicleResponse::from(vehicle)))
        .ok_or(ApiError::NotFound("vehicle"))
}

async fn vehicle_patch(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    body: Result<Json<UpdateVehicleRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<AcceptedResponse> {
    let vin = vin(&raw)?;
    let Json(request) = body?;
    let name = request
        .display_name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty());
    confirm(runtime(&state).set_vehicle_display_name(&vin, name)).await?;
    Ok(Json(AcceptedResponse::ok()))
}

async fn vehicle_enabled(
    State(state): State<AppState>,
    Path(raw): Path<String>,
    body: Result<Json<EnabledRequest>, axum::extract::rejection::JsonRejection>,
) -> ApiResult<AcceptedResponse> {
    let vin = vin(&raw)?;
    let Json(request) = body?;
    confirm(runtime(&state).set_vehicle_enabled(&vin, request.enabled)).await?;
    Ok(Json(AcceptedResponse::ok()))
}

async fn vehicle_refresh(
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> ApiResult<AcceptedResponse> {
    let vin = vin(&raw)?;
    confirm(runtime(&state).refresh_vehicle_now(&vin)).await?;
    Ok(Json(AcceptedResponse::ok()))
}

async fn vehicle_delete(
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> ApiResult<AcceptedResponse> {
    let vin = vin(&raw)?;
    confirm(runtime(&state).delete_vehicle(&vin)).await?;
    Ok(Json(AcceptedResponse::ok()))
}

async fn vehicle_state(
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> ApiResult<VehicleStateResponse> {
    let vin = vin(&raw)?;
    let vehicle = state
        .status()
        .vehicles
        .into_iter()
        .find(|vehicle| vehicle.vehicle.vin == vin)
        .ok_or(ApiError::NotFound("vehicle"))?;
    let snapshot = vehicle
        .last_state
        .ok_or(ApiError::NotFound("vehicle state"))?;
    Ok(Json(VehicleStateResponse {
        vin: vin.to_string(),
        state: snapshot,
    }))
}

/// The last raw manufacturer response, for the diagnostics page.
///
/// Always 404 today: storing raw responses belongs to the connector that fetches them, and no
/// connector does so yet.
async fn vehicle_raw(
    State(state): State<AppState>,
    Path(raw): Path<String>,
) -> ApiResult<serde_json::Value> {
    let vin = vin(&raw)?;
    if !state
        .status()
        .vehicles
        .iter()
        .any(|vehicle| vehicle.vehicle.vin == vin)
    {
        return Err(ApiError::NotFound("vehicle"));
    }
    Err(ApiError::NotFound("raw response"))
}
