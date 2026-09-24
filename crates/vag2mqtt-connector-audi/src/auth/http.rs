//! A thin wrapper over `reqwest` that records every exchange in the trace and never follows a
//! redirect on its own.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::cookie::Jar;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, StatusCode};
use url::Url;
use vag2mqtt_connector_api::ConnectorError;

use crate::cookies::Cookies;
use crate::error::{Step, network};
use crate::trace::Trace;

/// The `User-Agent` the reference poses as (`Docs/reference/audi-auth.md` section 2).
pub(crate) const USER_AGENT: &str = "myAudi-Android/4.14.1 (Build 800238275.2210271555) Android/11";

/// One HTTP response, body already read.
#[derive(Debug)]
pub struct Exchange {
    /// The URL that was requested (after our own relative resolution).
    pub url: Url,
    /// The status.
    pub status: StatusCode,
    /// Response headers.
    pub headers: HeaderMap,
    /// The body as text (lossy for binary).
    pub body: String,
}

impl Exchange {
    /// The `Location` header, if any.
    pub fn location(&self) -> Option<&str> {
        self.headers.get("location").and_then(|v| v.to_str().ok())
    }

    /// `true` for 301, 302, 303, 307, 308.
    pub fn is_redirect(&self) -> bool {
        self.status.is_redirection()
    }
}

/// A `reqwest::Client` with a cookie store, rustls, redirects disabled and a timeout.
///
/// Sending cookies is left to `reqwest`, which knows the domain and path rules. Every
/// `Set-Cookie` is additionally captured into a [`Cookies`] jar, because the EU Data Act portal's
/// session *is* a cookie and therefore has to outlive the client and be stored per account
/// (`Docs/roadmap-items/WP-25-audi-eu-data-act.md`).
#[derive(Clone, Debug)]
pub struct HttpClient {
    client: Client,
    captured: Arc<Mutex<Cookies>>,
}

impl HttpClient {
    /// Builds a client with an empty cookie jar.
    pub fn new() -> Result<Self, ConnectorError> {
        Self::build(Cookies::default())
    }

    /// Builds a client whose jar starts out holding `cookies`, to continue a stored session.
    pub(crate) fn with_cookies(cookies: Cookies) -> Result<Self, ConnectorError> {
        Self::build(cookies)
    }

    fn build(cookies: Cookies) -> Result<Self, ConnectorError> {
        let jar = Arc::new(Jar::default());
        for (domain, pair) in cookies.as_pairs() {
            // A cookie is restored onto the domain it was filed under; `Jar` needs a URL to
            // derive that domain from, and the scheme is always https here.
            if let Ok(url) = Url::parse(&format!("https://{domain}/")) {
                jar.add_cookie_str(&format!("{pair}; Path=/"), &url);
            }
        }
        let client = Client::builder()
            .use_rustls_tls()
            .cookie_provider(jar)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| ConnectorError::network("http client could not be built"))?;
        Ok(Self {
            client,
            captured: Arc::new(Mutex::new(cookies)),
        })
    }

    /// Every cookie seen so far, for storing the session.
    ///
    /// A poisoned lock yields an empty jar rather than a panic: the caller then treats the
    /// session as not established, which is the safe reading.
    pub(crate) fn cookies(&self) -> Cookies {
        self.captured
            .lock()
            .map(|jar| jar.clone())
            .unwrap_or_default()
    }

    /// The underlying client, for the vehicle API (WP-07).
    pub fn inner(&self) -> &Client {
        &self.client
    }

    /// A GET, traced.
    pub async fn get(
        &self,
        step: Step,
        url: Url,
        headers: &[(&str, &str)],
        trace: &mut Trace,
    ) -> Result<Exchange, ConnectorError> {
        self.send(step, Method::GET, url, headers, None, trace)
            .await
    }

    /// A form POST, traced.
    pub async fn post_form(
        &self,
        step: Step,
        url: Url,
        headers: &[(&str, &str)],
        form: &[(String, String)],
        trace: &mut Trace,
    ) -> Result<Exchange, ConnectorError> {
        let body = form
            .iter()
            .map(|(k, v)| {
                format!(
                    "{}={}",
                    crate::trace::form_encode(k),
                    crate::trace::form_encode(v)
                )
            })
            .collect::<Vec<_>>()
            .join("&");
        let mut all_headers: Vec<(&str, &str)> = headers.to_vec();
        all_headers.push(("Content-Type", "application/x-www-form-urlencoded"));
        self.send(step, Method::POST, url, &all_headers, Some(body), trace)
            .await
    }

    async fn send(
        &self,
        step: Step,
        method: Method,
        url: Url,
        headers: &[(&str, &str)],
        body: Option<String>,
        trace: &mut Trace,
    ) -> Result<Exchange, ConnectorError> {
        let mut header_map = HeaderMap::new();
        for (name, value) in headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                header_map.insert(name, value);
            }
        }
        let traced_headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        trace.request(
            step,
            method.as_str(),
            url.as_str(),
            &traced_headers,
            body.as_deref(),
        );
        tracing::trace!(target: "vag2mqtt::auth", step = %step, method = %method, host = url.host_str().unwrap_or("?"), path = url.path(), "request");

        let mut request = self.client.request(method, url.clone()).headers(header_map);
        if let Some(body) = body {
            request = request.body(body);
        }
        let response = request.send().await.map_err(|e| network(step, &e))?;
        let status = response.status();
        let response_headers = response.headers().clone();
        let final_url = response.url().clone();
        if let Ok(mut jar) = self.captured.lock() {
            jar.absorb(&final_url, &response_headers);
        }
        let body = response.text().await.map_err(|e| network(step, &e))?;

        let traced_response: Vec<(String, String)> = response_headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("[binary]").to_string()))
            .collect();
        trace.response(step, status.as_u16(), &traced_response, &body);
        tracing::trace!(target: "vag2mqtt::auth", step = %step, status = status.as_u16(), bytes = body.len(), "response");

        Ok(Exchange {
            url: final_url,
            status,
            headers: response_headers,
            body,
        })
    }
}

/// Resolves a `Location` header against the URL it was received from.
pub(crate) fn resolve_location(base: &Url, location: &str) -> Option<Url> {
    if let Ok(absolute) = Url::parse(location) {
        return Some(absolute);
    }
    base.join(location).ok()
}

#[cfg(test)]
mod tests {
    use reqwest::header::{HeaderValue, SET_COOKIE};
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::trace::TraceConfig;

    fn jar_with(url: &Url, cookie: &str) -> Cookies {
        let mut headers = HeaderMap::new();
        headers.append(SET_COOKIE, HeaderValue::from_str(cookie).unwrap());
        let mut jar = Cookies::default();
        jar.absorb(url, &headers);
        jar
    }

    /// The connector serves every Audi account through one object. Two clients built from two
    /// accounts' jars must each send only their own cookie.
    #[tokio::test]
    async fn clients_built_from_different_jars_do_not_share_cookies() {
        let server = MockServer::start().await;
        let base = Url::parse(&server.uri()).unwrap();
        for (cookie, body) in [("ACCOUNT=a", "a"), ("ACCOUNT=b", "b")] {
            Mock::given(method("GET"))
                .and(path("/whoami"))
                .and(header("cookie", cookie))
                .respond_with(ResponseTemplate::new(200).set_body_string(body))
                .mount(&server)
                .await;
        }
        let url = base.join("/whoami").unwrap();
        let mut trace = TraceConfig::disabled().begin("test");

        let a = HttpClient::with_cookies(jar_with(&base, "ACCOUNT=a")).unwrap();
        let b = HttpClient::with_cookies(jar_with(&base, "ACCOUNT=b")).unwrap();
        let fresh = HttpClient::new().unwrap();

        let answer = |exchange: Exchange| (exchange.status.as_u16(), exchange.body);
        assert_eq!(
            answer(
                a.get(Step::VehicleList, url.clone(), &[], &mut trace)
                    .await
                    .unwrap()
            ),
            (200, "a".to_string())
        );
        assert_eq!(
            answer(
                b.get(Step::VehicleList, url.clone(), &[], &mut trace)
                    .await
                    .unwrap()
            ),
            (200, "b".to_string())
        );
        // A client for a new login starts empty and matches neither account.
        assert_eq!(
            fresh
                .get(Step::VehicleList, url, &[], &mut trace)
                .await
                .unwrap()
                .status
                .as_u16(),
            404
        );
    }

    #[tokio::test]
    async fn every_set_cookie_is_captured_for_storage() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/login"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "/")
                    .insert_header("Set-Cookie", "SESSION=captured; Path=/; HttpOnly"),
            )
            .mount(&server)
            .await;
        let url = Url::parse(&server.uri()).unwrap().join("/login").unwrap();
        let client = HttpClient::new().unwrap();
        let mut trace = TraceConfig::disabled().begin("test");
        client
            .get(Step::PortalLanding, url, &[], &mut trace)
            .await
            .unwrap();
        let pairs = client.cookies().as_pairs();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].1, "SESSION=captured");
    }
}
