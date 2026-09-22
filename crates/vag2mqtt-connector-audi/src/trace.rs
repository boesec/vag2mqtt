//! Masked trace files for diagnosing a broken login flow.
//!
//! Enabled by the `VAG2MQTT_AUDI_TRACE=<directory>` environment variable, off when it is absent.
//! Nothing is written without it. Masking happens on the way out: header values of
//! `Authorization`, `Cookie` and `Set-Cookie`, JSON and form fields whose name contains `token`
//! or is `password`, `code_verifier`, `code` or `id_token`, anything shaped like a VIN, and the
//! account's user name.

use std::path::{Path, PathBuf};

use chrono::Utc;
use sha2::{Digest, Sha256};

use crate::error::Step;

/// The environment variable that names the trace directory.
pub const TRACE_ENV: &str = "VAG2MQTT_AUDI_TRACE";

const LOG: &str = "vag2mqtt::auth";

const README: &str = "These files describe one login attempt against your manufacturer account.\n\
Tokens, cookies, passwords, VINs and the user name are masked, but the pages and\n\
redirect chains still describe your account. Share them only with people you trust,\n\
and delete them when the problem is solved.\n";

/// Where traces go, if anywhere.
#[derive(Clone, Debug, Default)]
pub struct TraceConfig {
    directory: Option<PathBuf>,
}

impl TraceConfig {
    /// Tracing off.
    pub fn disabled() -> Self {
        Self { directory: None }
    }

    /// Tracing into `directory`.
    pub fn into_directory(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: Some(directory.into()),
        }
    }

    /// Reads [`TRACE_ENV`]. An empty value counts as unset.
    pub fn from_env() -> Self {
        match std::env::var(TRACE_ENV) {
            Ok(dir) if !dir.trim().is_empty() => Self::into_directory(dir.trim()),
            _ => Self::disabled(),
        }
    }

    /// `true` if traces are written.
    pub fn is_enabled(&self) -> bool {
        self.directory.is_some()
    }

    /// Starts a trace for one login attempt of `username`. Returns a no-op trace when disabled.
    pub fn begin(&self, username: &str) -> Trace {
        let Some(base) = &self.directory else {
            return Trace {
                dir: None,
                username: username.to_string(),
                summary: Vec::new(),
            };
        };
        let stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
        let dir = base.join(format!("{stamp}-{}", account_short(username)));
        match create_restricted_dir(&dir) {
            Ok(()) => {
                let _ = std::fs::write(dir.join("README.txt"), README);
                tracing::info!(target: LOG, path = %dir.display(), "writing masked auth trace");
                Trace {
                    dir: Some(dir),
                    username: username.to_string(),
                    summary: Vec::new(),
                }
            }
            Err(error) => {
                tracing::warn!(target: LOG, path = %dir.display(), %error, "could not create the trace directory; tracing off");
                Trace {
                    dir: None,
                    username: username.to_string(),
                    summary: Vec::new(),
                }
            }
        }
    }
}

/// Four hex characters derived from the user name, so the directory name carries no identity.
fn account_short(username: &str) -> String {
    let digest = Sha256::digest(username.as_bytes());
    digest.iter().take(2).map(|b| format!("{b:02x}")).collect()
}

#[cfg(unix)]
fn create_restricted_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_restricted_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// One login attempt's trace. Cheap when disabled.
pub struct Trace {
    dir: Option<PathBuf>,
    username: String,
    summary: Vec<String>,
}

impl Trace {
    /// `true` if files are written.
    pub fn is_enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// The trace directory, if enabled.
    pub fn directory(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    /// Records an outgoing request.
    pub fn request(
        &mut self,
        step: Step,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<&str>,
    ) {
        if self.dir.is_none() {
            return;
        }
        let mut text = format!("{method} {}\n", mask_text(url, &self.username));
        for (name, value) in headers {
            text.push_str(&self.header_line(name, value));
        }
        if let Some(body) = body {
            text.push('\n');
            text.push_str(&mask_body(body, &self.username));
            text.push('\n');
        }
        self.write(step, "request.txt", &text);
    }

    /// Records a response.
    pub fn response(&mut self, step: Step, status: u16, headers: &[(String, String)], body: &str) {
        if self.dir.is_none() {
            return;
        }
        let mut text = format!("HTTP {status}\n");
        for (name, value) in headers {
            text.push_str(&self.header_line(name, value));
        }
        text.push('\n');
        text.push_str(&mask_body(body, &self.username));
        text.push('\n');
        let name = if body.trim_start().starts_with('<') {
            "response.html"
        } else if body.trim_start().starts_with('{') {
            "response.json"
        } else {
            "response.txt"
        };
        self.write(step, name, &text);
    }

    /// One header line, fully masked: credential headers are replaced wholesale, and every other
    /// value still goes through [`mask_text`], because `Location` carries the authorization code.
    fn header_line(&self, name: &str, value: &str) -> String {
        let masked = mask_text(&mask_header(name, value), &self.username);
        format!("{name}: {masked}\n")
    }

    /// Records the outcome of a step in the summary.
    pub fn outcome(&mut self, step: Step, outcome: &str) {
        let line = format!(
            "{:02} {:<16} {}",
            step.index(),
            step.as_str(),
            mask_text(outcome, &self.username)
        );
        tracing::debug!(target: LOG, step = %step, outcome = %line, "auth step");
        self.summary.push(line);
    }

    /// Writes the summary. Called once at the end; a dropped trace writes nothing more.
    pub fn finish(&mut self) {
        let Some(dir) = &self.dir else {
            return;
        };
        let mut text = self.summary.join("\n");
        text.push('\n');
        let _ = std::fs::write(dir.join("summary.txt"), text);
    }

    fn write(&self, step: Step, name: &str, content: &str) {
        let Some(dir) = &self.dir else {
            return;
        };
        let mut counter = 0;
        loop {
            let suffix = if counter == 0 {
                String::new()
            } else {
                format!("-{counter}")
            };
            let path = dir.join(format!(
                "{:02}-{}{}-{}",
                step.index(),
                step.as_str(),
                suffix,
                name
            ));
            if path.exists() {
                counter += 1;
                continue;
            }
            if let Err(error) = std::fs::write(&path, content) {
                tracing::warn!(target: LOG, path = %path.display(), %error, "could not write trace file");
            }
            break;
        }
    }
}

/// Masks a header value where the header carries credentials.
pub(crate) fn mask_header(name: &str, value: &str) -> String {
    let lower = name.to_ascii_lowercase();
    if lower == "authorization" || lower == "cookie" || lower == "set-cookie" {
        "[REDACTED]".to_string()
    } else {
        value.to_string()
    }
}

/// `true` for field names whose values are secrets.
pub(crate) fn is_secret_field(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("token")
        || lower == "password"
        || lower == "code_verifier"
        || lower == "code"
        || lower == "id_token"
        || lower == "hmac"
        || lower == "_csrf"
        || lower == "client_secret"
        || lower == "authorization"
}

/// Masks a request or response body: JSON by field name, form encoded by field name, and in any
/// case VIN shapes and the user name.
pub(crate) fn mask_body(body: &str, username: &str) -> String {
    let trimmed = body.trim_start();
    let masked = if trimmed.starts_with('{') || trimmed.starts_with('[') {
        match serde_json::from_str::<serde_json::Value>(body) {
            Ok(mut value) => {
                mask_json(&mut value);
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| body.to_string())
            }
            Err(_) => body.to_string(),
        }
    } else if looks_form_encoded(body) {
        mask_form(body)
    } else {
        body.to_string()
    };
    mask_text(&masked, username)
}

/// Masks VIN shapes, the user name, and secret query parameters in URLs or fragments.
pub(crate) fn mask_text(text: &str, username: &str) -> String {
    let mut out = mask_query_params(text);
    if !username.is_empty() {
        out = out.replace(username, "[USERNAME]");
        let encoded = form_encode(username);
        if encoded != username {
            out = out.replace(&encoded, "[USERNAME]");
        }
    }
    mask_vins(&out)
}

fn mask_json(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, inner) in map.iter_mut() {
                if is_secret_field(key) {
                    *inner = serde_json::Value::String("[REDACTED]".into());
                } else {
                    mask_json(inner);
                }
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(mask_json),
        _ => {}
    }
}

fn looks_form_encoded(body: &str) -> bool {
    !body.contains('\n') && body.contains('=') && !body.contains('<')
}

fn mask_form(body: &str) -> String {
    body.split('&')
        .map(|pair| match pair.split_once('=') {
            Some((key, _)) if is_secret_field(key) => format!("{key}=[REDACTED]"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Masks `code=`, `*token*=` and friends inside URLs, fragments and HTML attribute values.
fn mask_query_params(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // A parameter name starts after '?', '&' or '#', or at the start of the text.
        let at_boundary = i == 0 || matches!(bytes[i - 1], b'?' | b'&' | b'#');
        if at_boundary {
            let name_end = text[i..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .map(|n| i + n)
                .unwrap_or(text.len());
            if name_end < text.len() && bytes[name_end] == b'=' {
                let name = &text[i..name_end];
                if is_secret_field(name) {
                    let value_end = text[name_end + 1..]
                        .find(['&', '#', '"', '\'', ' ', '\n', '<'])
                        .map(|n| name_end + 1 + n)
                        .unwrap_or(text.len());
                    out.push_str(name);
                    out.push_str("=[REDACTED]");
                    i = value_end;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap_or('\0');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Replaces every 17 character run of `[A-HJ-NPR-Z0-9]` bounded by non-alphanumerics with `[VIN]`.
fn mask_vins(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        let run_end = text[i..]
            .find(|c: char| !c.is_ascii_alphanumeric())
            .map(|n| i + n)
            .unwrap_or(text.len());
        let run = &text[i..run_end];
        if run.len() == 17
            && run.bytes().all(|b| {
                b.is_ascii_digit() || (b.is_ascii_uppercase() && !matches!(b, b'I' | b'O' | b'Q'))
            })
            && run.bytes().any(|b| b.is_ascii_uppercase())
        {
            out.push_str("[VIN]");
        } else {
            out.push_str(run);
        }
        if run_end < bytes.len() {
            let ch = text[run_end..].chars().next().unwrap_or('\0');
            out.push(ch);
            i = run_end + ch.len_utf8();
        } else {
            break;
        }
    }
    out
}

/// Minimal application/x-www-form-urlencoded encoding of a value, for matching the user name in
/// encoded bodies.
pub(crate) fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const USERNAME: &str = "driver@example.test";

    #[test]
    fn every_marker_is_masked() {
        let request_body = "email=driver%40example.test&password=hunter2-MARKER&_csrf=CSRF-MARKER&hmac=HMAC-MARKER&relayState=keep";
        let response_body = r#"{"access_token":"ACCESS-MARKER","refresh_token":"REFRESH-MARKER","id_token":"ID-MARKER","vehicles":[{"vin":"WAUZZZ1234567TEST"}],"expires_in":3600}"#;
        let url = "myaudi:///#state=abc&code=CODE-MARKER&id_token=ID-MARKER&x=1";
        let html = "<a href=\"https://x/cb?code=CODE-MARKER&amp;state=1\">driver@example.test WAUZZZ1234567TEST</a>";

        let masked_request = mask_body(request_body, USERNAME);
        let masked_response = mask_body(response_body, USERNAME);
        let masked_url = mask_text(url, USERNAME);
        let masked_html = mask_body(html, USERNAME);
        let masked_cookie = mask_header("Set-Cookie", "SESSION=COOKIE-MARKER");
        let masked_auth = mask_header("Authorization", "Bearer ACCESS-MARKER");

        for marker in [
            "hunter2",
            "CSRF-MARKER",
            "HMAC-MARKER",
            "ACCESS-MARKER",
            "REFRESH-MARKER",
            "ID-MARKER",
            "CODE-MARKER",
            "COOKIE-MARKER",
            "WAUZZZ1234567TEST",
            USERNAME,
            "driver%40example.test",
        ] {
            for text in [
                &masked_request,
                &masked_response,
                &masked_url,
                &masked_html,
                &masked_cookie,
                &masked_auth,
            ] {
                assert!(!text.contains(marker), "{marker} survived in {text}");
            }
        }
        assert!(masked_request.contains("relayState=keep"));
        assert!(masked_response.contains("\"expires_in\": 3600"));
        assert!(masked_url.contains("state=abc"));
        assert!(masked_html.contains("[VIN]"));
        assert!(masked_html.contains("[USERNAME]"));
        // A Location header carrying the authorization code must be masked too.
        let location = mask_text(
            &mask_header("Location", "myaudi:///#state=1&code=CODE-MARKER"),
            USERNAME,
        );
        assert!(!location.contains("CODE-MARKER"), "{location}");
    }

    #[test]
    fn plain_headers_and_short_runs_stay() {
        assert_eq!(mask_header("Content-Type", "text/html"), "text/html");
        assert_eq!(mask_text("hello WORLD 12345", ""), "hello WORLD 12345");
        assert_eq!(mask_text("12345678901234567", ""), "12345678901234567");
    }

    #[test]
    fn disabled_config_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let config = TraceConfig::disabled();
        let mut trace = config.begin(USERNAME);
        trace.request(Step::Authorize, "GET", "https://x", &[], None);
        trace.response(Step::Authorize, 200, &[], "<html>");
        trace.outcome(Step::Authorize, "ok");
        trace.finish();
        assert!(!trace.is_enabled());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn enabled_config_writes_masked_files() {
        let dir = tempfile::tempdir().unwrap();
        let config = TraceConfig::into_directory(dir.path());
        let mut trace = config.begin(USERNAME);
        trace.request(
            Step::TokenExchange,
            "POST",
            "https://x/token",
            &[("Cookie".into(), "SESSION=COOKIE-MARKER".into())],
            Some("code=CODE-MARKER&client_id=abc"),
        );
        trace.response(
            Step::TokenExchange,
            200,
            &[],
            r#"{"access_token":"ACCESS-MARKER"}"#,
        );
        trace.outcome(Step::TokenExchange, "ok");
        trace.finish();
        let attempt = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| p.is_dir())
            .unwrap();
        assert!(!attempt.to_string_lossy().contains("driver"));
        let mut all = String::new();
        for entry in std::fs::read_dir(&attempt).unwrap() {
            all.push_str(&std::fs::read_to_string(entry.unwrap().path()).unwrap());
        }
        assert!(all.contains("README") || all.contains("masked"));
        for marker in ["COOKIE-MARKER", "CODE-MARKER", "ACCESS-MARKER"] {
            assert!(!all.contains(marker), "{marker} leaked");
        }
        assert!(all.contains("07 token_exchange"));
    }
}
