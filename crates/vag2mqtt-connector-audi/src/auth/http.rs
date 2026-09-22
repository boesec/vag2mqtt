//! A thin wrapper over `reqwest` that records every exchange in the trace and never follows a
//! redirect on its own.

use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, StatusCode};
use url::Url;
use vag2mqtt_connector_api::ConnectorError;

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
#[derive(Clone, Debug)]
pub struct HttpClient {
    client: Client,
}

impl HttpClient {
    /// Builds the client.
    pub fn new() -> Result<Self, ConnectorError> {
        let client = Client::builder()
            .use_rustls_tls()
            .cookie_store(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .user_agent(USER_AGENT)
            .build()
            .map_err(|_| ConnectorError::network("http client could not be built"))?;
        Ok(Self { client })
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
