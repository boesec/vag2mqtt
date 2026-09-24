//! The classic form login: PKCE authorize, two scraped form posts, a manual redirect chase and
//! one token exchange. Parameters are documented in `Docs/reference/audi-auth.md` section 2.

use std::collections::BTreeMap;

use async_trait::async_trait;
use base64::Engine;
use chrono::{Duration as ChronoDuration, Utc};
use rand::RngCore;
use scraper::{Html, Selector};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use url::Url;
use vag2mqtt_connector_api::{ConnectorError, ConnectorInfo, Credentials, SessionState};
use vag2mqtt_domain::{Brand, DataSourceKind, Secret};

use super::AudiAuthStrategy;
use super::http::{Exchange, HttpClient, resolve_location};
use crate::error::{Step, classify_challenge_page, classify_known_error, manufacturer, parsing};
use crate::session::AudiTokens;
use crate::trace::Trace;

const LOG: &str = "vag2mqtt::auth";
const MAX_HOPS: usize = 12;

/// The hybrid flow: the callback fragment carries the tokens, so no exchange is needed.
///
/// The service advertises it under `response_types_supported`, but the myAudi app client is not
/// registered for it: the authorize request answers
/// `400 {"error":"invalid_request","error_description":"invalid client"}` (2026-09-23). Kept
/// because a differently registered client, such as the EU Data Act portal's, may accept it.
pub const HYBRID_RESPONSE_TYPE: &str = "code token id_token";

/// The classic flow: the callback carries a code that has to be exchanged.
pub const CODE_RESPONSE_TYPE: &str = "code";

/// The hosts and OAuth parameters of the flow. Tests point the hosts at a mock server.
#[derive(Clone, Debug)]
pub struct Endpoints {
    /// `https://identity.vwgroup.io`
    pub identity_base: Url,
    /// `https://emea.bff.cariad.digital`
    pub bff_base: Url,
    /// OAuth client id.
    pub client_id: String,
    /// The app callback the authorize request redirects to.
    pub redirect_uri: String,
    /// OAuth scopes.
    pub scope: String,
    /// The `x-client-id` header the backend expects.
    pub x_client_id: String,
    /// The OAuth `response_type`.
    ///
    /// `code` is the classic flow and the only one this client is registered for. The hybrid
    /// `code token id_token` would need no token exchange, but the authorize request rejects it
    /// with `invalid client`. Both dead ends are recorded in `Docs/reference/audi-auth.md`
    /// sections 1c and 1d.
    pub response_type: String,
    /// Whether the authorize request carries a PKCE challenge.
    ///
    /// The app route needs it. The EU Data Act portal must **not** get one: the portal exchanges
    /// the code itself without our verifier, so a challenge binds the code to a verifier the
    /// portal never sends, the exchange fails and the portal ends up without a session. That is
    /// what the first live portal test on 2026-09-24 ran into.
    pub pkce: bool,
    /// A fixed `state`, or `None` for a random one.
    ///
    /// The portal reads `state` as `<ui locale>__<data locale>__<brand>`, for Audi
    /// `de__en__AUDI`, so it cannot be random there.
    pub state: Option<String>,
    /// The `ui_locales` parameter, if any.
    pub ui_locales: Option<String>,
}

impl Default for Endpoints {
    fn default() -> Self {
        Self {
            identity_base: Url::parse("https://identity.vwgroup.io")
                .unwrap_or_else(|_| unreachable!()),
            bff_base: Url::parse("https://emea.bff.cariad.digital")
                .unwrap_or_else(|_| unreachable!()),
            client_id: "09b6cbec-cd19-4589-82fd-363dfa8c24da@apps_vw-dilab_com".into(),
            redirect_uri: "myaudi:///".into(),
            scope: "openid profile badge cars dealers vin".into(),
            x_client_id: "59edf286-a9ca-4d34-9421-68da00f72dc8".into(),
            response_type: CODE_RESPONSE_TYPE.into(),
            pkce: true,
            state: None,
            ui_locales: Some("de-DE en-US".into()),
        }
    }
}

impl Endpoints {
    fn authorize(&self) -> Url {
        let mut url = self.identity_base.clone();
        url.set_path("/oidc/v1/authorize");
        url
    }

    fn token(&self) -> Url {
        let mut url = self.bff_base.clone();
        url.set_path("/auth/v1/idk/oidc/token");
        url
    }
}

/// The steps both routes share: authorize, scrape the e-mail form, post it, scrape and post the
/// password form.
///
/// The native route and the EU Data Act portal differ only in their parameters and in what
/// happens after the password post, so everything up to that point lives here once.
#[derive(Clone, Debug)]
pub(crate) struct Signin {
    pub(crate) endpoints: Endpoints,
}

/// The classic form login, ending in a token exchange at the Cariad backend.
///
/// The exchange is refused for a public client (`Docs/reference/audi-auth.md` section 1c), so
/// this route does not currently reach tokens against the live service. It stays because the
/// portal route reuses four of its five steps, and because a reopened route would revive it.
#[derive(Clone, Debug)]
pub struct FormLoginStrategy {
    signin: Signin,
}

impl FormLoginStrategy {
    /// The production endpoints.
    pub fn new() -> Self {
        Self::with_endpoints(Endpoints::default())
    }

    /// Custom endpoints, for tests.
    pub fn with_endpoints(endpoints: Endpoints) -> Self {
        Self {
            signin: Signin { endpoints },
        }
    }
}

impl Default for FormLoginStrategy {
    fn default() -> Self {
        Self::new()
    }
}

/// PKCE verifier and challenge.
pub(crate) struct Pkce {
    verifier: Secret<String>,
    challenge: String,
}

impl Pkce {
    pub(crate) fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        Self::from_verifier(verifier)
    }

    fn from_verifier(verifier: String) -> Self {
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        Self {
            verifier: Secret::new(verifier),
            challenge,
        }
    }
}

fn random_hex(bytes: usize) -> String {
    let mut buffer = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buffer);
    buffer.iter().map(|b| format!("{b:02x}")).collect()
}

/// A scraped HTML form: where to post and which fields to carry over verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrapedForm {
    /// The form's `action`, as written.
    pub action: String,
    /// Every input with a name and a value, in document order.
    pub fields: BTreeMap<String, String>,
}

/// Finds the form for `step`: by id first (`emailPasswordForm`, `credentialsForm`), else the
/// first form whose action contains `action_hint`.
pub fn scrape_form(html: &str, ids: &[&str], action_hint: &str) -> Option<ScrapedForm> {
    let document = Html::parse_document(html);
    let form_selector = Selector::parse("form").ok()?;
    let input_selector = Selector::parse("input").ok()?;
    let mut candidates = document.select(&form_selector);
    let form = candidates.find(|form| {
        let id = form.value().attr("id").unwrap_or("");
        let action = form.value().attr("action").unwrap_or("");
        ids.contains(&id) || (!action_hint.is_empty() && action.contains(action_hint))
    })?;
    let action = form.value().attr("action")?.to_string();
    let mut fields = BTreeMap::new();
    for input in form.select(&input_selector) {
        let element = input.value();
        let Some(name) = element.attr("name") else {
            continue;
        };
        if matches!(element.attr("type"), Some("submit" | "button")) {
            continue;
        }
        fields.insert(
            name.to_string(),
            element.attr("value").unwrap_or("").to_string(),
        );
    }
    Some(ScrapedForm { action, fields })
}

/// The `templateModel` value inside a `window._IDK = {...}` block. Strict JSON, unlike the
/// JavaScript literal around it.
#[derive(Debug, Deserialize)]
struct IdkTemplate {
    hmac: Option<String>,
    #[serde(rename = "relayState")]
    relay_state: Option<String>,
    #[serde(rename = "postAction")]
    post_action: Option<String>,
    #[serde(rename = "emailPasswordForm")]
    email_password_form: Option<serde_json::Value>,
}

/// Extracts a form from a `window._IDK = {...};` script block.
///
/// The live block is a JavaScript object literal with unquoted keys and single quoted strings,
/// so it is **not** strict JSON. Only the `templateModel` value is, and that is the part that
/// carries `hmac` and `relayState`. The CSRF token is read out of the surrounding literal by
/// name. Used when a page renders its form by script instead of serving one.
pub fn scrape_idk_form(html: &str, client_id: &str, action_path: &str) -> Option<ScrapedForm> {
    let start = html.find("window._IDK")?;
    let block = &html[start..];
    let template_key = block.find("templateModel")?;
    let brace = block[template_key..].find('{')? + template_key;
    let json = balanced_json(&block[brace..])?;
    let template: IdkTemplate = serde_json::from_str(json).ok()?;
    let mut fields = BTreeMap::new();
    fields.insert("hmac".to_string(), template.hmac?);
    fields.insert("relayState".to_string(), template.relay_state?);
    if let Some(csrf) = javascript_string_field(block, "csrf_token") {
        fields.insert("_csrf".to_string(), csrf);
    }
    let action = match template.post_action {
        // The live service sends `"postAction":"login/authenticate"`, relative to the client's
        // sign-in root and **not** to the page URL. Resolving it against the page, whose path
        // already ends in `/login/authenticate`, produces `/login/login/authenticate` and a
        // HTTP 400. Observed on 2026-09-23; see `Docs/reference/audi-auth.md` section 1b.
        Some(action) if is_absolute_action(&action) => action,
        Some(relative) => format!("/signin-service/v1/{client_id}/{relative}"),
        None => format!("/signin-service/v1/{client_id}{action_path}"),
    };
    let _ = template.email_password_form;
    Some(ScrapedForm { action, fields })
}

/// `true` for an action that already names its own place: an absolute path or a full URL.
fn is_absolute_action(action: &str) -> bool {
    action.starts_with('/') || action.contains("://")
}

/// Reads `name: VALUE` out of a JavaScript object literal, where VALUE is a quoted string.
///
/// Tolerates both spellings the service uses: a bare key as in `csrf_token: 'abc'` and a quoted
/// one as in `"csrf_token": "abc"`.
fn javascript_string_field(text: &str, name: &str) -> Option<String> {
    let after_key = [format!("{name}:"), format!("{name}\":")]
        .iter()
        .filter_map(|key| find_at_word_boundary(text, key).map(|at| at + key.len()))
        .min()?;
    let rest = &text[after_key..];
    // Only whitespace may sit between the colon and the opening quote.
    let quote = rest.find(['\'', '"'])?;
    if !rest[..quote].chars().all(char::is_whitespace) {
        return None;
    }
    let delimiter = rest[quote..].chars().next()?;
    let value_start = quote + delimiter.len_utf8();
    let end = rest[value_start..].find(delimiter)? + value_start;
    Some(rest[value_start..end].to_string())
}

/// Finds `needle` where the character before it is not part of an identifier, so looking for
/// `n":` does not match the tail of `csrf_token":`.
fn find_at_word_boundary(text: &str, needle: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = text[from..].find(needle) {
        let at = from + offset;
        let preceding = text[..at].chars().next_back();
        let is_boundary = preceding.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if is_boundary {
            return Some(at);
        }
        from = at + needle.len();
    }
    None
}

/// Returns the JSON object starting at `text[0] == '{'`, honouring nesting and strings.
fn balanced_json(text: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&text[..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every parameter of the app callback URL, from both the fragment and the query.
///
/// The classic flow answers in the query (`myaudi:///?code=...`), the hybrid flow in the
/// fragment (`myaudi:///#access_token=...&id_token=...`). Reading both means the parser does not
/// have to know which flow produced the callback.
pub fn callback_params(callback: &str) -> BTreeMap<String, String> {
    let after_scheme = callback
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(callback);
    let mut params = BTreeMap::new();
    let (before_fragment, fragment) = match after_scheme.split_once('#') {
        Some((before, fragment)) => (before, Some(fragment)),
        None => (after_scheme, None),
    };
    let query = before_fragment.split_once('?').map(|(_, q)| q);
    for part in [query, fragment].into_iter().flatten() {
        for pair in part.split('&') {
            if let Some((key, value)) = pair.split_once('=')
                && !value.is_empty()
            {
                params.insert(key.to_string(), percent_decode(value));
            }
        }
    }
    params
}

/// Decodes `%XX` escapes and `+`. Tokens are base64url and need no decoding, but a `redirect_uri`
/// echoed back does.
fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&value[i + 1..i + 3], 16) {
                Ok(byte) => {
                    out.push(byte);
                    i += 3;
                }
                Err(_) => {
                    out.push(bytes[i]);
                    i += 1;
                }
            },
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Reads the authorization code from the app callback URL.
pub fn code_from_callback(callback: &str) -> Option<String> {
    callback_params(callback).remove("code")
}

/// Builds tokens from a hybrid flow callback, if it carries an access token.
///
/// The fragment has no refresh token: renewing would need the token endpoint, which accepts
/// only `client_secret_basic` or `client_secret_post`.
pub fn tokens_from_callback(callback: &str, strategy: &str) -> Option<AudiTokens> {
    let params = callback_params(callback);
    let access = params.get("access_token")?;
    let expires_in = params
        .get("expires_in")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(3600)
        .max(60);
    Some(AudiTokens {
        access: Secret::new(access.clone()),
        refresh: params.get("refresh_token").cloned().map(Secret::new),
        id_token: params.get("id_token").cloned().map(Secret::new),
        expires_at: Utc::now() + ChronoDuration::seconds(expires_in),
        strategy: strategy.to_string(),
    })
}

/// The token endpoint's answer.
#[derive(Debug, Deserialize)]
pub struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
    error: Option<String>,
    error_description: Option<String>,
}

/// Parses a token response body into tokens. `previous_refresh` fills in a refresh token the
/// backend did not repeat.
pub fn parse_token_response(
    body: &str,
    strategy: &str,
    previous_refresh: Option<&Secret<String>>,
) -> Result<AudiTokens, ConnectorError> {
    let response: TokenResponse =
        serde_json::from_str(body).map_err(|_| parsing(Step::TokenExchange))?;
    if let Some(error) = &response.error {
        tracing::warn!(target: LOG, error, description = response.error_description.as_deref().unwrap_or(""), "token endpoint returned an error");
        return Err(ConnectorError::Manufacturer {
            status: Some(400),
            code: Some(error.clone()),
        });
    }
    let access = response
        .access_token
        .ok_or_else(|| parsing(Step::TokenExchange))?;
    let refresh = response
        .refresh_token
        .map(Secret::new)
        .or_else(|| previous_refresh.cloned());
    let expires_in = response.expires_in.unwrap_or(3600).max(60);
    Ok(AudiTokens {
        access: Secret::new(access),
        refresh,
        id_token: response.id_token.map(Secret::new),
        expires_at: Utc::now() + ChronoDuration::seconds(expires_in),
        strategy: strategy.to_string(),
    })
}

#[async_trait]
impl AudiAuthStrategy for FormLoginStrategy {
    fn name(&self) -> &'static str {
        "form"
    }

    fn info(&self) -> ConnectorInfo {
        ConnectorInfo {
            brand: Brand::Audi,
            kind: DataSourceKind::LiveApi,
            min_polling_interval: crate::connector::MIN_LIVE_POLLING_INTERVAL,
            supports_commands: false,
        }
    }

    async fn login(
        &self,
        credentials: &Credentials,
        trace: &mut Trace,
    ) -> Result<SessionState, ConnectorError> {
        let http = HttpClient::new()?;
        let result = self.run_login(&http, credentials, trace).await;
        match &result {
            Ok(tokens) => trace.outcome(
                Step::TokenExchange,
                &format!(
                    "ok, access token valid until {}",
                    tokens.expires_at.to_rfc3339()
                ),
            ),
            Err(error) => trace.outcome(Step::TokenExchange, &format!("failed: {error}")),
        }
        trace.finish();
        result.map(AudiTokens::into_session)
    }

    async fn refresh(
        &self,
        session: &mut SessionState,
        trace: &mut Trace,
    ) -> Result<(), ConnectorError> {
        let tokens = AudiTokens::from_session(session)?;
        let http = HttpClient::new()?;
        let result = self.run_refresh(&http, &tokens, trace).await;
        match &result {
            Ok(_) => trace.outcome(Step::Refresh, "ok"),
            Err(error) => trace.outcome(Step::Refresh, &format!("failed: {error}")),
        }
        trace.finish();
        *session = result?.into_session();
        Ok(())
    }
}

impl FormLoginStrategy {
    async fn run_refresh(
        &self,
        http: &HttpClient,
        tokens: &AudiTokens,
        trace: &mut Trace,
    ) -> Result<AudiTokens, ConnectorError> {
        let Some(refresh_token) = &tokens.refresh else {
            // The hybrid flow yields no refresh token, so the runtime has to log in again. It
            // does exactly that on `SessionExpired`, with the stored password (WP-05).
            tracing::info!(
                target: LOG,
                "this session has no refresh token; a new login is needed"
            );
            return Err(ConnectorError::SessionExpired);
        };
        let endpoints = &self.signin.endpoints;
        let form = vec![
            ("grant_type".to_string(), "refresh_token".to_string()),
            (
                "refresh_token".to_string(),
                refresh_token.expose_secret().clone(),
            ),
            ("client_id".to_string(), endpoints.client_id.clone()),
        ];
        let headers = [
            ("Accept", "application/json"),
            ("x-client-id", endpoints.x_client_id.as_str()),
        ];
        let exchange = http
            .post_form(Step::Refresh, endpoints.token(), &headers, &form, trace)
            .await?;
        if exchange.status.is_success() {
            parse_token_response(&exchange.body, self.name(), tokens.refresh.as_ref())
        } else if exchange.status.is_server_error() {
            Err(manufacturer(exchange.status.as_u16(), None))
        } else {
            tracing::info!(target: LOG, status = exchange.status.as_u16(), "refresh rejected");
            Err(ConnectorError::SessionExpired)
        }
    }

    async fn run_login(
        &self,
        http: &HttpClient,
        credentials: &Credentials,
        trace: &mut Trace,
    ) -> Result<AudiTokens, ConnectorError> {
        let pkce = Pkce::generate();
        let posted = self.signin.sign_in(http, credentials, &pkce, trace).await?;

        // Step 5: chase redirects until the app callback.
        let callback = self.signin.chase_to_callback(http, posted, trace).await?;
        trace.outcome(Step::RedirectChase, "reached the app callback");

        // Step 6: what the callback carries.
        if let Some(tokens) = tokens_from_callback(&callback, self.name()) {
            trace.outcome(
                Step::Callback,
                "tokens received in the callback (hybrid flow), no exchange needed",
            );
            return Ok(tokens);
        }
        let code = code_from_callback(&callback).ok_or_else(|| parsing(Step::Callback))?;
        trace.outcome(Step::Callback, "authorization code received, exchanging it");

        // Step 7: tokens.
        let endpoints = &self.signin.endpoints;
        let form = vec![
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code".to_string(), code),
            ("redirect_uri".to_string(), endpoints.redirect_uri.clone()),
            ("client_id".to_string(), endpoints.client_id.clone()),
            (
                "code_verifier".to_string(),
                pkce.verifier.expose_secret().clone(),
            ),
        ];
        let headers = [
            ("Accept", "application/json"),
            ("x-client-id", endpoints.x_client_id.as_str()),
        ];
        let exchange = http
            .post_form(
                Step::TokenExchange,
                endpoints.token(),
                &headers,
                &form,
                trace,
            )
            .await?;
        if exchange.status.is_server_error() {
            return Err(manufacturer(exchange.status.as_u16(), None));
        }
        if !exchange.status.is_success() {
            let code = serde_json::from_str::<TokenResponse>(&exchange.body)
                .ok()
                .and_then(|r| r.error);
            return Err(manufacturer(exchange.status.as_u16(), code));
        }
        parse_token_response(&exchange.body, self.name(), None)
    }
}

impl Signin {
    /// Steps 1 to 4, shared by both routes: authorize, e-mail form, identifier post, password
    /// post. Returns the password post's response, which is where the routes diverge.
    pub(crate) async fn sign_in(
        &self,
        http: &HttpClient,
        credentials: &Credentials,
        pkce: &Pkce,
        trace: &mut Trace,
    ) -> Result<Exchange, ConnectorError> {
        let state = self
            .endpoints
            .state
            .clone()
            .unwrap_or_else(|| random_hex(16));
        let nonce = random_hex(16);

        // Step 1: authorize, chased to the sign-in page.
        let mut authorize = self.endpoints.authorize();
        {
            let mut query = authorize.query_pairs_mut();
            query
                .append_pair("response_type", &self.endpoints.response_type)
                .append_pair("client_id", &self.endpoints.client_id)
                .append_pair("redirect_uri", &self.endpoints.redirect_uri)
                .append_pair("scope", &self.endpoints.scope)
                .append_pair("state", &state)
                .append_pair("nonce", &nonce)
                .append_pair("prompt", "login");
            if let Some(locales) = &self.endpoints.ui_locales {
                query.append_pair("ui_locales", locales);
            }
            if self.endpoints.pkce {
                query
                    .append_pair("code_challenge", &pkce.challenge)
                    .append_pair("code_challenge_method", "S256");
            }
        }
        let page = self
            .chase_to_page(http, Step::Authorize, authorize, trace)
            .await?;
        trace.outcome(
            Step::Authorize,
            &format!("HTTP {} at {}", page.status.as_u16(), page.url.path()),
        );
        if !page.status.is_success() {
            return Err(self.unexpected(Step::Authorize, &page));
        }

        // Step 2: the e-mail form.
        let email_form = scrape_form(&page.body, &["emailPasswordForm"], "/login/identifier")
            .or_else(|| scrape_idk_form(&page.body, &self.endpoints.client_id, "/login/identifier"))
            .ok_or_else(|| self.page_error(Step::SigninForm, &page))?;
        trace.outcome(
            Step::SigninForm,
            &format!("form with {} fields", email_form.fields.len()),
        );
        let mut fields: Vec<(String, String)> = email_form.fields.into_iter().collect();
        upsert(&mut fields, "email", &credentials.username);
        let action = resolve_location(&page.url, &email_form.action)
            .ok_or_else(|| parsing(Step::SigninForm))?;

        // Step 3: post it and chase to the password page.
        let posted = http
            .post_form(Step::IdentifierPost, action, &[], &fields, trace)
            .await?;
        self.check_known_errors(&posted)?;
        let password_page = self
            .chase_from(http, Step::IdentifierPost, posted, trace)
            .await?;
        trace.outcome(
            Step::IdentifierPost,
            &format!(
                "HTTP {} at {}",
                password_page.status.as_u16(),
                password_page.url.path()
            ),
        );
        if !password_page.status.is_success() {
            return Err(self.unexpected(Step::IdentifierPost, &password_page));
        }

        // Step 4: the password form (plain form or the window._IDK model).
        let password_form = scrape_form(
            &password_page.body,
            &["credentialsForm"],
            "/login/authenticate",
        )
        .or_else(|| {
            scrape_idk_form(
                &password_page.body,
                &self.endpoints.client_id,
                "/login/authenticate",
            )
        })
        .ok_or_else(|| self.page_error(Step::PasswordPost, &password_page))?;
        trace.outcome(
            Step::PasswordPost,
            &format!("form with {} fields", password_form.fields.len()),
        );
        let mut fields: Vec<(String, String)> = password_form.fields.into_iter().collect();
        upsert(&mut fields, "email", &credentials.username);
        upsert(
            &mut fields,
            "password",
            credentials.password.expose_secret(),
        );
        let action = resolve_location(&password_page.url, &password_form.action)
            .ok_or_else(|| parsing(Step::PasswordPost))?;
        let posted = http
            .post_form(Step::PasswordPost, action, &[], &fields, trace)
            .await?;
        self.check_known_errors(&posted)?;
        // An error here belongs to the password post, not to the chase that would follow it.
        // Naming the wrong step sent the 2026-09-23 spike looking in the wrong place.
        if !posted.is_redirect() && !posted.status.is_success() {
            trace.outcome(
                Step::PasswordPost,
                &format!(
                    "HTTP {} where a redirect was expected",
                    posted.status.as_u16()
                ),
            );
            return Err(self.unexpected(Step::PasswordPost, &posted));
        }
        Ok(posted)
    }

    /// Follows redirects starting with a GET of `url` until a non-redirect answer.
    pub(crate) async fn chase_to_page(
        &self,
        http: &HttpClient,
        step: Step,
        url: Url,
        trace: &mut Trace,
    ) -> Result<Exchange, ConnectorError> {
        let first = http.get(step, url, &[], trace).await?;
        self.chase_from(http, step, first, trace).await
    }

    /// Follows redirects from an exchange until a non-redirect answer. A hop to the app callback
    /// is an error here: the caller expected a page.
    pub(crate) async fn chase_from(
        &self,
        http: &HttpClient,
        step: Step,
        mut current: Exchange,
        trace: &mut Trace,
    ) -> Result<Exchange, ConnectorError> {
        for _ in 0..MAX_HOPS {
            if !current.is_redirect() {
                return Ok(current);
            }
            let location = current.location().ok_or_else(|| parsing(step))?.to_string();
            self.check_known_errors_in(&location)?;
            if location.starts_with(&self.endpoints.redirect_uri) {
                return Err(parsing(step));
            }
            let next = resolve_location(&current.url, &location).ok_or_else(|| parsing(step))?;
            current = http.get(step, next, &[], trace).await?;
        }
        Err(parsing(step))
    }

    /// Follows redirects after the password post into the portal, and stops at the first page
    /// the portal itself serves.
    ///
    /// Unlike [`chase_to_callback`](Self::chase_to_callback) this does **not** stop at the
    /// redirect URI: that request is the one the portal answers with its session cookie, so
    /// stopping there would throw the session away. Consent pages are only posted while the
    /// chase is still at the identity service; once it is on `portal`, whatever is served is
    /// the application and is returned as it is.
    ///
    /// `portal` is an authority (`host` or `host:port`), not a bare host, so that a mock
    /// identity service and a mock portal on the same loopback address stay distinguishable.
    pub(crate) async fn chase_into_portal(
        &self,
        http: &HttpClient,
        portal: &str,
        mut current: Exchange,
        trace: &mut Trace,
    ) -> Result<Exchange, ConnectorError> {
        let step = Step::PortalLanding;
        for _ in 0..MAX_HOPS {
            if current.is_redirect() {
                let location = current.location().ok_or_else(|| parsing(step))?.to_string();
                self.check_known_errors_in(&location)?;
                let next =
                    resolve_location(&current.url, &location).ok_or_else(|| parsing(step))?;
                current = http.get(step, next, &[], trace).await?;
                continue;
            }
            if !current.status.is_success() {
                return Err(self.unexpected(step, &current));
            }
            let on_portal = current.url.authority() == portal;
            if !on_portal {
                self.check_known_errors(&current)?;
                if let Some(challenge) = classify_challenge_page(&current.body) {
                    trace.outcome(step, &format!("challenge page: {challenge}"));
                    return Err(challenge);
                }
                if let Some(form) = scrape_form(&current.body, &[], "terms")
                    .or_else(|| scrape_form(&current.body, &[], "consent"))
                {
                    trace.outcome(step, "consent page, accepting");
                    let fields: Vec<(String, String)> = form.fields.into_iter().collect();
                    let action = resolve_location(&current.url, &form.action)
                        .ok_or_else(|| parsing(step))?;
                    current = http.post_form(step, action, &[], &fields, trace).await?;
                    continue;
                }
                trace.outcome(
                    step,
                    &format!(
                        "HTTP 200 page at {} where the portal was expected",
                        current.url.path()
                    ),
                );
                return Err(parsing(step));
            }
            let path = current.url.path();
            if ["signin-service", "/consent", "/error"]
                .iter()
                .any(|marker| path.contains(marker))
            {
                trace.outcome(
                    step,
                    &format!("landed on {path}, which is not the portal application"),
                );
                return Err(ConnectorError::Manufacturer {
                    status: Some(current.status.as_u16()),
                    code: Some(format!("the portal login ended at {path}")),
                });
            }
            trace.outcome(step, &format!("landed on the portal at {path}"));
            return Ok(current);
        }
        Err(parsing(step))
    }

    /// Follows redirects after the password post until the app callback, handling a consent
    /// page on the way.
    async fn chase_to_callback(
        &self,
        http: &HttpClient,
        mut current: Exchange,
        trace: &mut Trace,
    ) -> Result<String, ConnectorError> {
        let step = Step::RedirectChase;
        for _ in 0..MAX_HOPS {
            if current.is_redirect() {
                let location = current.location().ok_or_else(|| parsing(step))?.to_string();
                self.check_known_errors_in(&location)?;
                if location.starts_with(&self.endpoints.redirect_uri) {
                    return Ok(location);
                }
                let next =
                    resolve_location(&current.url, &location).ok_or_else(|| parsing(step))?;
                current = http.get(step, next, &[], trace).await?;
                continue;
            }
            if current.status.is_success() {
                self.check_known_errors(&current)?;
                if let Some(challenge) = classify_challenge_page(&current.body) {
                    trace.outcome(step, &format!("challenge page: {challenge}"));
                    return Err(challenge);
                }
                // A terms and conditions or consent page: post it as it is and continue.
                if let Some(form) = scrape_form(&current.body, &[], "terms")
                    .or_else(|| scrape_form(&current.body, &[], "consent"))
                {
                    trace.outcome(step, "consent page, accepting");
                    let fields: Vec<(String, String)> = form.fields.into_iter().collect();
                    let action = resolve_location(&current.url, &form.action)
                        .ok_or_else(|| parsing(step))?;
                    current = http.post_form(step, action, &[], &fields, trace).await?;
                    continue;
                }
                trace.outcome(
                    step,
                    &format!(
                        "HTTP 200 page at {} where a redirect was expected",
                        current.url.path()
                    ),
                );
                return Err(parsing(step));
            }
            return Err(self.unexpected(step, &current));
        }
        Err(parsing(step))
    }

    pub(crate) fn check_known_errors(&self, exchange: &Exchange) -> Result<(), ConnectorError> {
        if let Some(location) = exchange.location() {
            self.check_known_errors_in(location)?;
        }
        self.check_known_errors_in(&exchange.body)
    }

    pub(crate) fn check_known_errors_in(&self, text: &str) -> Result<(), ConnectorError> {
        match classify_known_error(text) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// A page that could not be parsed: a known error first, a challenge second, else parsing.
    pub(crate) fn page_error(&self, step: Step, page: &Exchange) -> ConnectorError {
        if let Some(known) = classify_known_error(&page.body) {
            return known;
        }
        if let Some(challenge) = classify_challenge_page(&page.body) {
            return challenge;
        }
        parsing(step)
    }

    /// An unexpected status.
    pub(crate) fn unexpected(&self, step: Step, page: &Exchange) -> ConnectorError {
        if let Some(known) = classify_known_error(&page.body) {
            return known;
        }
        let status = page.status.as_u16();
        if page.status.is_server_error() {
            return manufacturer(status, None);
        }
        if status == 429 {
            return ConnectorError::RateLimited { retry_after: None };
        }
        tracing::warn!(target: LOG, step = %step, status, "unexpected status");
        parsing(step)
    }
}

fn upsert(fields: &mut Vec<(String, String)>, key: &str, value: &str) {
    match fields.iter_mut().find(|(k, _)| k == key) {
        Some(entry) => entry.1 = value.to_string(),
        None => fields.push((key.to_string(), value.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/audi/auth")
            .join(name);
        std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("fixture {}", path.display()))
    }

    #[test]
    fn pkce_challenge_matches_rfc_7636_vector() {
        let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(
            pkce.challenge,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let generated = Pkce::generate();
        assert_eq!(generated.verifier.expose_secret().len(), 43);
    }

    #[test]
    fn email_form_is_scraped_with_hidden_fields_verbatim() {
        let form = scrape_form(
            &fixture("signin_email_form.html"),
            &["emailPasswordForm"],
            "/login/identifier",
        )
        .unwrap();
        assert_eq!(
            form.action,
            "/signin-service/v1/09b6cbec-cd19-4589-82fd-363dfa8c24da@apps_vw-dilab_com/login/identifier"
        );
        assert_eq!(
            form.fields.get("_csrf").map(String::as_str),
            Some("csrf-fixture")
        );
        assert_eq!(
            form.fields.get("relayState").map(String::as_str),
            Some("relay-fixture")
        );
        assert_eq!(
            form.fields.get("hmac").map(String::as_str),
            Some("hmac-fixture")
        );
        assert_eq!(
            form.fields.get("registerFlow").map(String::as_str),
            Some("false")
        );
        assert!(form.fields.contains_key("email"));
        assert!(
            !form.fields.contains_key("next"),
            "submit buttons are not fields"
        );
    }

    #[test]
    fn password_form_is_scraped_from_plain_form_and_from_idk_model() {
        let plain = scrape_form(
            &fixture("signin_password_form.html"),
            &["credentialsForm"],
            "/login/authenticate",
        )
        .unwrap();
        assert!(plain.action.ends_with("/login/authenticate"));
        assert_eq!(
            plain.fields.get("hmac").map(String::as_str),
            Some("hmac-fixture-2")
        );
        assert!(plain.fields.contains_key("password"));

        let idk = scrape_idk_form(
            &fixture("signin_password_idk.html"),
            "client-fixture",
            "/login/authenticate",
        )
        .unwrap();
        assert_eq!(
            idk.action,
            "/signin-service/v1/client-fixture/login/authenticate"
        );
        assert_eq!(
            idk.fields.get("hmac").map(String::as_str),
            Some("hmac-idk-fixture")
        );
        assert_eq!(
            idk.fields.get("relayState").map(String::as_str),
            Some("relay-idk-fixture")
        );
        assert_eq!(
            idk.fields.get("_csrf").map(String::as_str),
            Some("csrf-idk-fixture")
        );
    }

    #[test]
    fn renamed_form_yields_none_and_the_caller_reports_parsing() {
        let html = fixture("signin_email_form.html")
            .replace("emailPasswordForm", "somethingElse")
            .replace("/login/identifier", "/login/other");
        assert_eq!(
            scrape_form(&html, &["emailPasswordForm"], "/login/identifier"),
            None
        );
        assert_eq!(
            parsing(Step::SigninForm),
            ConnectorError::Parsing {
                context: "signin_form"
            }
        );
        assert_eq!(
            scrape_idk_form("<html></html>", "x", "/login/authenticate"),
            None
        );
    }

    #[test]
    fn code_is_read_from_fragment_or_query() {
        assert_eq!(
            code_from_callback("myaudi:///#state=1&code=abc&id_token=x"),
            Some("abc".into())
        );
        assert_eq!(
            code_from_callback("myaudi:///?code=def&state=1"),
            Some("def".into())
        );
        assert_eq!(code_from_callback("myaudi:///#state=1"), None);
        assert_eq!(code_from_callback("myaudi:///"), None);
    }

    #[test]
    fn callback_parameters_come_from_query_and_fragment() {
        let hybrid = "myaudi:///#state=abc&code=C&access_token=A&id_token=I&expires_in=3600&token_type=bearer";
        let params = callback_params(hybrid);
        assert_eq!(params.get("access_token").map(String::as_str), Some("A"));
        assert_eq!(params.get("id_token").map(String::as_str), Some("I"));
        assert_eq!(params.get("code").map(String::as_str), Some("C"));
        assert_eq!(params.get("expires_in").map(String::as_str), Some("3600"));

        // The classic flow answers in the query, which is what the service sent on 2026-09-23.
        let classic = "myaudi:///?state=ae2d&code=C";
        assert_eq!(
            callback_params(classic).get("code").map(String::as_str),
            Some("C")
        );

        // Empty values are not parameters, and escapes are decoded.
        let odd = "myaudi:///?a=&b=x%20y&c=p+q";
        let params = callback_params(odd);
        assert!(!params.contains_key("a"));
        assert_eq!(params.get("b").map(String::as_str), Some("x y"));
        assert_eq!(params.get("c").map(String::as_str), Some("p q"));
    }

    #[test]
    fn hybrid_callback_yields_tokens_without_an_exchange() {
        let callback = "myaudi:///#state=abc&access_token=ACCESS&id_token=ID&expires_in=1800&token_type=bearer";
        let tokens = tokens_from_callback(callback, "form").expect("the fragment carries tokens");
        assert_eq!(tokens.access.expose_secret(), "ACCESS");
        assert_eq!(
            tokens.id_token.as_ref().map(|t| t.expose_secret().as_str()),
            Some("ID")
        );
        assert!(
            tokens.refresh.is_none(),
            "the hybrid flow yields no refresh token"
        );
        let lifetime = tokens.expires_at - Utc::now();
        assert!(lifetime.num_seconds() > 1700 && lifetime.num_seconds() <= 1800);

        // A callback with only a code is not a hybrid answer.
        assert!(tokens_from_callback("myaudi:///?code=C&state=1", "form").is_none());
    }

    #[test]
    fn the_default_response_type_is_the_only_registered_one() {
        // The hybrid flow is rejected with `invalid client` by this client id, so the classic
        // flow stays the default even though its exchange is closed to us.
        assert_eq!(Endpoints::default().response_type, CODE_RESPONSE_TYPE);
        assert_eq!(HYBRID_RESPONSE_TYPE, "code token id_token");
        assert_eq!(CODE_RESPONSE_TYPE, "code");
    }

    #[test]
    fn token_response_is_parsed_and_errors_surface() {
        let tokens = parse_token_response(&fixture("token_response.json"), "form", None).unwrap();
        assert_eq!(tokens.access.expose_secret(), "access-fixture");
        assert_eq!(
            tokens.refresh.as_ref().map(|t| t.expose_secret().as_str()),
            Some("refresh-fixture")
        );
        assert!(tokens.id_token.is_some());
        assert!(tokens.expires_at > Utc::now() + ChronoDuration::seconds(3000));

        // A response without a refresh token is fine; the runtime logs in again when the
        // access token expires.
        let without_refresh = r#"{"access_token":"a","expires_in":100}"#;
        assert!(
            parse_token_response(without_refresh, "form", None)
                .unwrap()
                .refresh
                .is_none()
        );
        let previous = Secret::new("old-refresh".to_string());
        let kept = parse_token_response(without_refresh, "form", Some(&previous)).unwrap();
        assert_eq!(
            kept.refresh.as_ref().map(|t| t.expose_secret().as_str()),
            Some("old-refresh")
        );

        let error = parse_token_response(
            r#"{"error":"invalid_grant","error_description":"x"}"#,
            "form",
            None,
        )
        .unwrap_err();
        assert_eq!(
            error,
            ConnectorError::Manufacturer {
                status: Some(400),
                code: Some("invalid_grant".into())
            }
        );
        assert!(matches!(
            parse_token_response("not json", "form", None),
            Err(ConnectorError::Parsing { .. })
        ));
    }

    /// The page the service really served on 2026-09-22, anonymised. Guards against fixtures
    /// that only match our own invented markup.
    #[test]
    fn the_live_sign_in_page_is_scraped() {
        let html = fixture("live_signin_identifier_2026-09-22.html");
        let form = scrape_form(&html, &["emailPasswordForm"], "/login/identifier")
            .expect("the live page carries a real form element");
        assert_eq!(
            form.action,
            "/signin-service/v1/09b6cbec-cd19-4589-82fd-363dfa8c24da@apps_vw-dilab_com/login/identifier"
        );
        assert_eq!(
            form.fields.get("_csrf").map(String::as_str),
            Some("csrf-live-fixture")
        );
        assert_eq!(
            form.fields.get("relayState").map(String::as_str),
            Some("relay-live-fixture")
        );
        assert_eq!(
            form.fields.get("hmac").map(String::as_str),
            Some("hmac-live-fixture")
        );
        assert!(
            form.fields.contains_key("email"),
            "the e-mail input is carried over"
        );
        assert!(
            !form.fields.contains_key("next-btn"),
            "the submit button is not a field"
        );
        assert_eq!(form.fields.len(), 4, "{:?}", form.fields);

        // The same page carries a window._IDK literal; the fallback must read it, and that
        // literal is JavaScript, not JSON.
        let idk = scrape_idk_form(
            &html,
            "09b6cbec-cd19-4589-82fd-363dfa8c24da@apps_vw-dilab_com",
            "/login/identifier",
        )
        .expect("the _IDK fallback reads the live block");
        assert_eq!(
            idk.fields.get("hmac").map(String::as_str),
            Some("hmac-live-fixture")
        );
        assert_eq!(
            idk.fields.get("relayState").map(String::as_str),
            Some("relay-live-fixture")
        );
        assert_eq!(
            idk.fields.get("_csrf").map(String::as_str),
            Some("csrf-live-fixture")
        );
        assert!(idk.action.ends_with("/login/identifier"));

        // No CAPTCHA and no second factor on the page the service serves today.
        assert_eq!(classify_challenge_page(&html), None);
    }

    /// The password page the service really served on 2026-09-23, anonymised.
    ///
    /// It renders its form by script, and its `postAction` is relative to the client's sign-in
    /// root rather than to the page URL. Resolving it against the page produced
    /// `/login/login/authenticate` and a HTTP 400; this test pins the fix.
    #[test]
    fn the_live_password_page_resolves_its_action_correctly() {
        let html = fixture("live_signin_password_2026-09-23.html");
        assert!(
            scrape_form(&html, &["credentialsForm"], "/login/authenticate").is_none(),
            "the live password page carries no form element at all"
        );

        let form = scrape_idk_form(
            &html,
            "09b6cbec-cd19-4589-82fd-363dfa8c24da@apps_vw-dilab_com",
            "/login/authenticate",
        )
        .expect("the _IDK fallback carries the password page");
        assert_eq!(
            form.action,
            "/signin-service/v1/09b6cbec-cd19-4589-82fd-363dfa8c24da@apps_vw-dilab_com/login/authenticate",
            "a relative postAction resolves against the sign-in root, not against the page"
        );
        assert_eq!(
            form.fields.get("hmac").map(String::as_str),
            Some("hmac-live-fixture")
        );
        assert_eq!(
            form.fields.get("relayState").map(String::as_str),
            Some("relay-live-fixture")
        );
        assert_eq!(
            form.fields.get("_csrf").map(String::as_str),
            Some("csrf-live-fixture")
        );

        // And the resolved URL is the one the service accepts, not the doubled path.
        let page_url = Url::parse(
            "https://identity.vwgroup.io/signin-service/v1/09b6cbec-cd19-4589-82fd-363dfa8c24da@apps_vw-dilab_com/login/authenticate",
        )
        .expect("valid page url");
        let resolved = resolve_location(&page_url, &form.action).expect("resolves");
        assert_eq!(
            resolved.path(),
            "/signin-service/v1/09b6cbec-cd19-4589-82fd-363dfa8c24da@apps_vw-dilab_com/login/authenticate"
        );
        assert!(!resolved.path().contains("/login/login/"), "{resolved}");

        assert_eq!(classify_challenge_page(&html), None);
    }

    #[test]
    fn an_absolute_post_action_is_left_alone() {
        assert!(is_absolute_action(
            "/signin-service/v1/x/login/authenticate"
        ));
        assert!(is_absolute_action("https://identity.example.test/x"));
        assert!(!is_absolute_action("login/authenticate"));
    }

    #[test]
    fn javascript_string_fields_are_read_from_a_literal() {
        let literal = "{ a: 1, csrf_token: 'abc-123', b: \"x\" }";
        assert_eq!(
            javascript_string_field(literal, "csrf_token"),
            Some("abc-123".into())
        );
        assert_eq!(javascript_string_field(literal, "b"), Some("x".into()));
        assert_eq!(javascript_string_field(literal, "missing"), None);
        // The quoted spelling, as strict JSON writes it.
        let json_style = "{\"csrf_token\": \"abc-123\", \"n\": 1}";
        assert_eq!(
            javascript_string_field(json_style, "csrf_token"),
            Some("abc-123".into())
        );
        // A number is not a string, so nothing is read.
        assert_eq!(javascript_string_field(json_style, "n"), None);
    }

    #[test]
    fn balanced_json_handles_nesting_and_strings() {
        assert_eq!(
            balanced_json(r#"{"a":{"b":"}"},"c":1}; tail"#),
            Some(r#"{"a":{"b":"}"},"c":1}"#)
        );
        assert_eq!(balanced_json("{unterminated"), None);
    }
}
