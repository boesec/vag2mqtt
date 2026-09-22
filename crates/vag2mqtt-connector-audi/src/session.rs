//! The Audi session: tokens and their expiry, stored inside `SessionState::payload`.

use chrono::{DateTime, Utc};
use vag2mqtt_connector_api::{ConnectorError, SessionState};
use vag2mqtt_domain::{Brand, Secret};

/// Safety margin before the access token's real expiry at which the runtime refreshes.
pub(crate) const REFRESH_MARGIN_SECS: i64 = 60;

/// Tokens of a myAudi session.
#[derive(Clone)]
pub struct AudiTokens {
    /// The bearer token for API calls.
    pub access: Secret<String>,
    /// The refresh token.
    pub refresh: Secret<String>,
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
            "refresh_token": self.refresh.expose_secret(),
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
            refresh: Secret::new(string("refresh_token")?),
            id_token: payload
                .get("id_token")
                .and_then(|v| v.as_str())
                .map(|t| Secret::new(t.to_string())),
            expires_at,
            strategy: string("strategy").unwrap_or_else(|_| "form".to_string()),
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
            refresh: Secret::new("REFRESH-MARKER".into()),
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
        assert_eq!(back.refresh.expose_secret(), "REFRESH-MARKER");
        assert_eq!(back.expires_at, expires_at);
        assert_eq!(back.strategy, "form");
        assert!(!format!("{back:?}").contains("MARKER"));
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
}
