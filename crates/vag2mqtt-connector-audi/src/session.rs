//! The Audi session, stored inside `SessionState::payload`.
//!
//! There are two shapes, because there are two routes: the native route keeps tokens, the EU
//! Data Act portal keeps a cookie jar. Both are treated as secret by persistence, which
//! encrypts the whole payload.

use chrono::{DateTime, Utc};
use vag2mqtt_connector_api::{ConnectorError, SessionState};
use vag2mqtt_domain::{Brand, Secret};

use crate::cookies::Cookies;

/// The name the portal route stores in its session payload.
pub(crate) const EU_DATA_ACT_ROUTE: &str = "eu_data_act";

/// Safety margin before the access token's real expiry at which the runtime refreshes.
pub(crate) const REFRESH_MARGIN_SECS: i64 = 60;

/// Tokens of a myAudi session.
#[derive(Clone)]
pub struct AudiTokens {
    /// The bearer token for API calls.
    pub access: Secret<String>,
    /// The refresh token, when the flow yields one.
    ///
    /// The hybrid flow does not: renewing needs the token endpoint, and that endpoint accepts
    /// only `client_secret_basic` or `client_secret_post`, which a public client cannot serve.
    /// `None` therefore means "a full login is the only way to renew".
    pub refresh: Option<Secret<String>>,
    /// The OpenID Connect identity token, if the backend delivered one.
    pub id_token: Option<Secret<String>>,
    /// When the access token expires.
    pub expires_at: DateTime<Utc>,
    /// Name of the strategy that produced the tokens (`form`, later `device_code`).
    pub strategy: String,
}

impl std::fmt::Debug for AudiTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudiTokens")
            .field("expires_at", &self.expires_at)
            .field("strategy", &self.strategy)
            .field("tokens", &"[REDACTED]")
            .finish()
    }
}

impl AudiTokens {
    /// Serialises into a `SessionState`, with `expires_at` moved forward by the refresh margin.
    ///
    /// This is the one place where the tokens leave their wrappers, so persistence can encrypt
    /// them.
    pub fn into_session(self) -> SessionState {
        let payload = serde_json::json!({
            "access_token": self.access.expose_secret(),
            "refresh_token": self.refresh.as_ref().map(|t| t.expose_secret().clone()),
            "id_token": self.id_token.as_ref().map(|t| t.expose_secret().clone()),
            "expires_at": self.expires_at.to_rfc3339(),
            "strategy": self.strategy,
        });
        SessionState {
            brand: Brand::Audi,
            payload,
            expires_at: Some(self.expires_at - chrono::Duration::seconds(REFRESH_MARGIN_SECS)),
        }
    }

    /// Reads the tokens back from a `SessionState`.
    pub fn from_session(session: &SessionState) -> Result<Self, ConnectorError> {
        let parsing = || ConnectorError::Parsing {
            context: "stored audi session",
        };
        if session.brand != Brand::Audi {
            return Err(parsing());
        }
        let payload = &session.payload;
        let string = |key: &str| -> Result<String, ConnectorError> {
            payload
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .ok_or_else(parsing)
        };
        let expires_at = DateTime::parse_from_rfc3339(&string("expires_at")?)
            .map(|t| t.with_timezone(&Utc))
            .map_err(|_| parsing())?;
        Ok(Self {
            access: Secret::new(string("access_token")?),
            refresh: string("refresh_token").ok().map(Secret::new),
            id_token: payload
                .get("id_token")
                .and_then(|v| v.as_str())
                .map(|t| Secret::new(t.to_string())),
            expires_at,
            strategy: string("strategy").unwrap_or_else(|_| "form".to_string()),
        })
    }
}

/// The EU Data Act portal's session: a cookie jar and when it was established.
///
/// There is no expiry, because the portal never states one. The route finds out that the
/// session is gone by being answered `401` or `403`, which it maps onto
/// [`ConnectorError::SessionExpired`] so the supervisor logs in again (WP-05).
#[derive(Clone)]
pub(crate) struct PortalSession {
    /// Everything the portal and the identity service set along the way.
    pub(crate) cookies: Cookies,
    /// When the login that produced these cookies finished.
    pub(crate) established_at: DateTime<Utc>,
}

impl std::fmt::Debug for PortalSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PortalSession")
            .field("established_at", &self.established_at)
            .field("cookies", &format!("{} [REDACTED]", self.cookies.len()))
            .finish()
    }
}

impl PortalSession {
    /// A session from a jar captured just now.
    pub(crate) fn new(cookies: Cookies) -> Self {
        Self {
            cookies,
            established_at: Utc::now(),
        }
    }

    /// Serialises into a `SessionState` so persistence can encrypt it.
    pub(crate) fn into_session(self) -> SessionState {
        SessionState {
            brand: Brand::Audi,
            payload: serde_json::json!({
                "route": EU_DATA_ACT_ROUTE,
                "cookies": self.cookies,
                "established_at": self.established_at.to_rfc3339(),
            }),
            expires_at: None,
        }
    }

    /// Reads the jar back out of a `SessionState`.
    pub(crate) fn from_session(session: &SessionState) -> Result<Self, ConnectorError> {
        let parsing = || ConnectorError::Parsing {
            context: "stored audi portal session",
        };
        if session.brand != Brand::Audi {
            return Err(parsing());
        }
        if session.payload.get("route").and_then(|v| v.as_str()) != Some(EU_DATA_ACT_ROUTE) {
            return Err(parsing());
        }
        let cookies: Cookies = session
            .payload
            .get("cookies")
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .ok_or_else(parsing)?;
        let established_at = session
            .payload
            .get("established_at")
            .and_then(|v| v.as_str())
            .and_then(|v| DateTime::parse_from_rfc3339(v).ok())
            .map(|t| t.with_timezone(&Utc))
            .ok_or_else(parsing)?;
        Ok(Self {
            cookies,
            established_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_round_trip_and_margin() {
        let expires_at = Utc::now() + chrono::Duration::hours(1);
        let tokens = AudiTokens {
            access: Secret::new("ACCESS-MARKER".into()),
            refresh: Some(Secret::new("REFRESH-MARKER".into())),
            id_token: None,
            expires_at,
            strategy: "form".into(),
        };
        let session = tokens.into_session();
        assert_eq!(
            session.expires_at,
            Some(expires_at - chrono::Duration::seconds(60))
        );
        assert!(!format!("{session:?}").contains("MARKER"));
        let back = AudiTokens::from_session(&session).unwrap();
        assert_eq!(back.access.expose_secret(), "ACCESS-MARKER");
        assert_eq!(
            back.refresh.as_ref().map(|t| t.expose_secret().as_str()),
            Some("REFRESH-MARKER")
        );
        assert_eq!(back.expires_at, expires_at);
        assert_eq!(back.strategy, "form");
        assert!(!format!("{back:?}").contains("MARKER"));
    }

    #[test]
    fn a_session_without_a_refresh_token_round_trips() {
        let tokens = AudiTokens {
            access: Secret::new("ACCESS-MARKER".into()),
            refresh: None,
            id_token: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
            strategy: "form".into(),
        };
        let session = tokens.into_session();
        let back = AudiTokens::from_session(&session).unwrap();
        assert!(back.refresh.is_none(), "no refresh token survives as None");
        assert_eq!(back.access.expose_secret(), "ACCESS-MARKER");
    }

    #[test]
    fn garbage_payload_is_a_parsing_error() {
        let session = SessionState::new(Brand::Audi, serde_json::json!({"nope": 1}));
        assert!(matches!(
            AudiTokens::from_session(&session),
            Err(ConnectorError::Parsing { .. })
        ));
        let wrong_brand = SessionState::new(Brand::Skoda, serde_json::json!({}));
        assert!(AudiTokens::from_session(&wrong_brand).is_err());
    }
    #[test]
    fn a_portal_session_round_trips_and_hides_its_cookies() {
        let mut cookies = Cookies::default();
        cookies.absorb(
            &url::Url::parse("https://portal.example.test/login").unwrap(),
            &cookie_header("SESSION=COOKIE-MARKER; Path=/"),
        );
        let portal = PortalSession::new(cookies);
        assert!(!format!("{portal:?}").contains("MARKER"));
        let session = portal.clone().into_session();
        assert!(session.expires_at.is_none(), "the portal states no expiry");
        let back = PortalSession::from_session(&session).unwrap();
        assert_eq!(back.cookies, portal.cookies);
        assert_eq!(back.established_at, portal.established_at);
    }

    #[test]
    fn a_token_session_is_not_mistaken_for_a_portal_session() {
        let tokens = AudiTokens {
            access: Secret::new("ACCESS-MARKER".into()),
            refresh: None,
            id_token: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
            strategy: "form".into(),
        };
        let session = tokens.into_session();
        assert!(PortalSession::from_session(&session).is_err());
    }

    fn cookie_header(value: &str) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.append(
            reqwest::header::SET_COOKIE,
            reqwest::header::HeaderValue::from_str(value).unwrap(),
        );
        headers
    }
}
