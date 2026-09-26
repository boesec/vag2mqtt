//! Steps of the login flow and the mapping of what we observe onto `ConnectorError`.

use vag2mqtt_connector_api::ConnectorError;

/// The named steps of an authentication flow. Every error names the step it happened in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    /// The PKCE authorize request and the redirect to the sign-in page.
    Authorize,
    /// Scraping the e-mail form from the sign-in page.
    SigninForm,
    /// Posting the e-mail form.
    IdentifierPost,
    /// Scraping and posting the password form.
    PasswordPost,
    /// Following the redirects after the password post.
    RedirectChase,
    /// Reading the authorization code from the app callback.
    Callback,
    /// Exchanging the code for tokens.
    TokenExchange,
    /// Refreshing tokens.
    Refresh,
    /// Landing on the EU Data Act portal after the redirect chase.
    PortalLanding,
    /// Asking the portal which vehicles the account has.
    VehicleList,
    /// Asking the portal for a vehicle's data request identifier.
    DataRequest,
    /// Listing a vehicle's export packages.
    DatasetList,
    /// Downloading one export package.
    DatasetDownload,
}

impl Step {
    /// The step's name, used in error contexts and trace file names.
    pub fn as_str(self) -> &'static str {
        match self {
            Step::Authorize => "authorize",
            Step::SigninForm => "signin_form",
            Step::IdentifierPost => "identifier_post",
            Step::PasswordPost => "password_post",
            Step::RedirectChase => "redirect_chase",
            Step::Callback => "callback",
            Step::TokenExchange => "token_exchange",
            Step::Refresh => "refresh",
            Step::PortalLanding => "portal_landing",
            Step::VehicleList => "vehicle_list",
            Step::DataRequest => "data_request",
            Step::DatasetList => "dataset_list",
            Step::DatasetDownload => "dataset_download",
        }
    }

    /// The number used as the prefix of the step's trace files.
    pub fn index(self) -> u8 {
        match self {
            Step::Authorize => 1,
            Step::SigninForm => 2,
            Step::IdentifierPost => 3,
            Step::PasswordPost => 4,
            Step::RedirectChase => 5,
            Step::Callback => 6,
            Step::TokenExchange => 7,
            Step::Refresh => 8,
            // The two routes share steps 1 to 5 and then diverge, and only one route's steps
            // ever land in the same trace directory, so the portal reuses 6 and 7.
            Step::PortalLanding => 6,
            Step::VehicleList => 7,
            // A fetch writes its own trace directory, so these start after the login's steps.
            Step::DataRequest => 9,
            Step::DatasetList => 10,
            Step::DatasetDownload => 11,
        }
    }
}

impl std::fmt::Display for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Known error identifiers of the VW Group identity service.
pub(crate) const PASSWORD_INVALID: &str = "login.errors.password_invalid";
/// Known error identifiers of the VW Group identity service.
pub(crate) const EMAIL_INVALID: &str = "validator.email.invalid";
/// Known error identifiers of the VW Group identity service.
pub(crate) const THROTTLED: &str = "login.error.throttled";

/// Maps a known identity service error identifier, if `text` contains one.
///
/// Known strings map before any status handling, so a throttled account becomes a rate limit
/// and not a credential error (which would stop polling until the user acts).
pub(crate) fn classify_known_error(text: &str) -> Option<ConnectorError> {
    if text.contains(THROTTLED) {
        return Some(ConnectorError::RateLimited { retry_after: None });
    }
    if text.contains(PASSWORD_INVALID) || text.contains(EMAIL_INVALID) {
        return Some(ConnectorError::InvalidCredentials);
    }
    None
}

/// Best effort detection of a page that asks for something we cannot provide.
///
/// Unknown shapes are **not** guessed: the caller reports `Parsing` with the step instead, so a
/// wrong guess never masquerades as a credential error.
pub(crate) fn classify_challenge_page(html: &str) -> Option<ConnectorError> {
    let lower = html.to_ascii_lowercase();
    if lower.contains("captcha") {
        return Some(ConnectorError::CaptchaRequired);
    }
    let two_factor_markers = [
        "one-time password",
        "one time password",
        "verification code",
        "second factor",
        "two-factor",
        "two factor",
        "name=\"otp\"",
        "name=\"mfa\"",
        "totp",
    ];
    if two_factor_markers.iter().any(|m| lower.contains(m)) {
        return Some(ConnectorError::TwoFactorRequired);
    }
    None
}

/// A parsing failure in `step`.
pub(crate) fn parsing(step: Step) -> ConnectorError {
    ConnectorError::Parsing {
        context: step.as_str(),
    }
}

/// Maps a transport error without leaking the URL.
pub(crate) fn network(step: Step, error: &reqwest::Error) -> ConnectorError {
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection failed"
    } else if error.is_body() || error.is_decode() {
        "body could not be read"
    } else if error.is_request() {
        "request could not be sent"
    } else {
        "transport error"
    };
    ConnectorError::network(format!("{step}: {kind}"))
}

/// Maps an unexpected HTTP status from the backend.
pub(crate) fn manufacturer(status: u16, code: Option<String>) -> ConnectorError {
    ConnectorError::Manufacturer {
        status: Some(status),
        code,
    }
}

#[cfg(test)]
mod tests {
    use vag2mqtt_connector_api::ErrorClass;

    use super::*;

    #[test]
    fn known_error_strings_map_before_status() {
        assert_eq!(
            classify_known_error("?error=login.errors.password_invalid"),
            Some(ConnectorError::InvalidCredentials)
        );
        assert_eq!(
            classify_known_error("validator.email.invalid"),
            Some(ConnectorError::InvalidCredentials)
        );
        assert_eq!(
            classify_known_error("login.error.throttled"),
            Some(ConnectorError::RateLimited { retry_after: None })
        );
        assert_eq!(classify_known_error("all good"), None);
    }

    #[test]
    fn challenge_pages_are_detected_conservatively() {
        assert_eq!(
            classify_challenge_page("<div class=\"g-recaptcha\"></div>"),
            Some(ConnectorError::CaptchaRequired)
        );
        assert_eq!(
            classify_challenge_page("<input name=\"otp\"> enter the verification code"),
            Some(ConnectorError::TwoFactorRequired)
        );
        assert_eq!(
            classify_challenge_page("<html><body>hello</body></html>"),
            None
        );
    }

    #[test]
    fn mapping_table_rows_have_the_expected_class() {
        let rows: Vec<(ConnectorError, ErrorClass)> = vec![
            (ConnectorError::InvalidCredentials, ErrorClass::AuthInvalid),
            (
                ConnectorError::RateLimited { retry_after: None },
                ErrorClass::RateLimited,
            ),
            (ConnectorError::TwoFactorRequired, ErrorClass::AuthInvalid),
            (ConnectorError::CaptchaRequired, ErrorClass::AuthInvalid),
            (ConnectorError::SessionExpired, ErrorClass::SessionExpired),
            (parsing(Step::RedirectChase), ErrorClass::Permanent),
            (
                ConnectorError::network("authorize: timeout"),
                ErrorClass::Transient,
            ),
            (manufacturer(503, None), ErrorClass::Transient),
        ];
        for (error, class) in rows {
            assert_eq!(error.class(), class, "{error}");
        }
        assert_eq!(
            parsing(Step::Callback).to_string(),
            "could not parse the manufacturer response (callback)"
        );
    }
}
