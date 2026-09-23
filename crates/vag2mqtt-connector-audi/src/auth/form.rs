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
use vag2mqtt_connector_api::{ConnectorError, Credentials};
use vag2mqtt_domain::Secret;

use super::AudiAuthStrategy;
use super::http::{Exchange, HttpClient, resolve_location};
use crate::error::{Step, classify_challenge_page, classify_known_error, manufacturer, parsing};
use crate::session::AudiTokens;
use crate::trace::Trace;

const LOG: &str = "vag2mqtt::auth";
const MAX_HOPS: usize = 12;

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

/// The classic form login.
#[derive(Clone, Debug)]
pub struct FormLoginStrategy {
    endpoints: Endpoints,
}

impl FormLoginStrategy {
    /// The production endpoints.
    pub fn new() -> Self {
        Self::with_endpoints(Endpoints::default())
    }

    /// Custom endpoints, for tests.
    pub fn with_endpoints(endpoints: Endpoints) -> Self {
        Self { endpoints }
    }
}

impl Default for FormLoginStrategy {
    fn default() -> Self {
        Self::new()
    }
}

/// PKCE verifier and challenge.
struct Pkce {
    verifier: Secret<String>,
    challenge: String,
}

impl Pkce {
    fn generate() -> Self {
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

/// Reads the authorization code from the app callback URL (`myaudi:///#code=...` or `?code=`).
pub fn code_from_callback(callback: &str) -> Option<String> {
    let after_scheme = callback
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(callback);
    let params = after_scheme
        .split_once('#')
        .map(|(_, f)| f)
        .or_else(|| after_scheme.split_once('?').map(|(_, q)| q))?;
    params.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == "code" && !value.is_empty()).then(|| value.to_string())
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
        .or_else(|| previous_refresh.cloned())
        .ok_or_else(|| parsing(Step::TokenExchange))?;
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

    async fn login(
        &self,
        http: &HttpClient,
        credentials: &Credentials,
        trace: &mut Trace,
    ) -> Result<AudiTokens, ConnectorError> {
        let result = self.run_login(http, credentials, trace).await;
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
        result
    }

    async fn refresh(
        &self,
        http: &HttpClient,
        tokens: &AudiTokens,
        trace: &mut Trace,
    ) -> Result<AudiTokens, ConnectorError> {
        let form = vec![
            ("grant_type".to_string(), "refresh_token".to_string()),
            (
                "refresh_token".to_string(),
                tokens.refresh.expose_secret().clone(),
            ),
            ("client_id".to_string(), self.endpoints.client_id.clone()),
        ];
        let headers = [
            ("Accept", "application/json"),
            ("x-client-id", self.endpoints.x_client_id.as_str()),
        ];
        let exchange = http
            .post_form(
                Step::Refresh,
                self.endpoints.token(),
                &headers,
                &form,
                trace,
            )
            .await?;
        let result = if exchange.status.is_success() {
            parse_token_response(&exchange.body, self.name(), Some(&tokens.refresh))
        } else if exchange.status.is_server_error() {
            Err(manufacturer(exchange.status.as_u16(), None))
        } else {
            tracing::info!(target: LOG, status = exchange.status.as_u16(), "refresh rejected");
            Err(ConnectorError::SessionExpired)
        };
        match &result {
            Ok(_) => trace.outcome(Step::Refresh, "ok"),
            Err(error) => trace.outcome(Step::Refresh, &format!("failed: {error}")),
        }
        trace.finish();
        result
    }
}

impl FormLoginStrategy {
    async fn run_login(
        &self,
        http: &HttpClient,
        credentials: &Credentials,
        trace: &mut Trace,
    ) -> Result<AudiTokens, ConnectorError> {
        let pkce = Pkce::generate();
        let state = random_hex(16);
        let nonce = random_hex(16);

        // Step 1: authorize, chased to the sign-in page.
        let mut authorize = self.endpoints.authorize();
        authorize
            .query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.endpoints.client_id)
            .append_pair("redirect_uri", &self.endpoints.redirect_uri)
            .append_pair("scope", &self.endpoints.scope)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("prompt", "login")
            .append_pair("ui_locales", "de-DE en-US")
            .append_pair("code_challenge", &pkce.challenge)
            .append_pair("code_challenge_method", "S256");
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

        // Step 5: chase redirects until the app callback.
        let callback = self.chase_to_callback(http, posted, trace).await?;
        trace.outcome(Step::RedirectChase, "reached the app callback");

        // Step 6: the code.
        let code = code_from_callback(&callback).ok_or_else(|| parsing(Step::Callback))?;
        trace.outcome(Step::Callback, "authorization code received");

        // Step 7: tokens.
        let form = vec![
            ("grant_type".to_string(), "authorization_code".to_string()),
            ("code".to_string(), code),
            (
                "redirect_uri".to_string(),
                self.endpoints.redirect_uri.clone(),
            ),
            ("client_id".to_string(), self.endpoints.client_id.clone()),
            (
                "code_verifier".to_string(),
                pkce.verifier.expose_secret().clone(),
            ),
        ];
        let headers = [
            ("Accept", "application/json"),
            ("x-client-id", self.endpoints.x_client_id.as_str()),
        ];
        let exchange = http
            .post_form(
                Step::TokenExchange,
                self.endpoints.token(),
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

    /// Follows redirects starting with a GET of `url` until a non-redirect answer.
    async fn chase_to_page(
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
    async fn chase_from(
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

    fn check_known_errors(&self, exchange: &Exchange) -> Result<(), ConnectorError> {
        if let Some(location) = exchange.location() {
            self.check_known_errors_in(location)?;
        }
        self.check_known_errors_in(&exchange.body)
    }

    fn check_known_errors_in(&self, text: &str) -> Result<(), ConnectorError> {
        match classify_known_error(text) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// A page that could not be parsed: a known error first, a challenge second, else parsing.
    fn page_error(&self, step: Step, page: &Exchange) -> ConnectorError {
        if let Some(known) = classify_known_error(&page.body) {
            return known;
        }
        if let Some(challenge) = classify_challenge_page(&page.body) {
            return challenge;
        }
        parsing(step)
    }

    /// An unexpected status.
    fn unexpected(&self, step: Step, page: &Exchange) -> ConnectorError {
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
    fn token_response_is_parsed_and_errors_surface() {
        let tokens = parse_token_response(&fixture("token_response.json"), "form", None).unwrap();
        assert_eq!(tokens.access.expose_secret(), "access-fixture");
        assert_eq!(tokens.refresh.expose_secret(), "refresh-fixture");
        assert!(tokens.id_token.is_some());
        assert!(tokens.expires_at > Utc::now() + ChronoDuration::seconds(3000));

        let without_refresh = r#"{"access_token":"a","expires_in":100}"#;
        assert!(matches!(
            parse_token_response(without_refresh, "form", None),
            Err(ConnectorError::Parsing { .. })
        ));
        let previous = Secret::new("old-refresh".to_string());
        let kept = parse_token_response(without_refresh, "form", Some(&previous)).unwrap();
        assert_eq!(kept.refresh.expose_secret(), "old-refresh");

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
