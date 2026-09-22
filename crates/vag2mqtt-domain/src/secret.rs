//! A wrapper for sensitive values that never prints its content.

use std::fmt;

use serde::{Deserialize, Deserializer};
use zeroize::{Zeroize, Zeroizing};

/// A secret value: password, token, cookie, PKCE verifier.
///
/// - `Debug` and `Display` print a redaction marker, never the content.
/// - `Deserialize` is implemented so that persistence and the API can construct one.
/// - `Serialize` is deliberately **not** implemented. Whoever writes a secret out calls
///   [`Secret::expose_secret`] explicitly, so every plaintext exit is greppable.
/// - No `PartialEq` and no `Hash`, so a secret is never compared or keyed by accident.
/// - The content is zeroised when the wrapper is dropped.
///
/// ```compile_fail
/// let secret = vag2mqtt_domain::Secret::new(String::from("hunter2"));
/// let _ = serde_json::to_string(&secret);
/// ```
#[derive(Clone)]
pub struct Secret<T: Zeroize>(Zeroizing<T>);

impl<'de, T: Zeroize + Deserialize<'de>> Deserialize<'de> for Secret<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::new)
    }
}

impl<T: Zeroize> Secret<T> {
    /// Wraps a value.
    pub fn new(value: T) -> Self {
        Self(Zeroizing::new(value))
    }

    /// Grants access to the content. Call sites of this method are the audit trail for
    /// where secrets leave their wrapper.
    pub fn expose_secret(&self) -> &T {
        &self.0
    }
}

impl<T: Zeroize> From<T> for Secret<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

impl<T: Zeroize> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([REDACTED])")
    }
}

impl<T: Zeroize> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAINTEXT: &str = "correct-horse-battery-staple";

    #[test]
    fn debug_and_display_never_show_the_content() {
        let secret = Secret::new(PLAINTEXT.to_string());
        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert_eq!(debug, "Secret([REDACTED])");
        assert_eq!(display, "[REDACTED]");
        for window in PLAINTEXT.as_bytes().windows(4) {
            let fragment = std::str::from_utf8(window).unwrap();
            assert!(!debug.contains(fragment));
            assert!(!display.contains(fragment));
        }
        assert_eq!(secret.expose_secret(), PLAINTEXT);
    }

    #[test]
    fn deserialises_from_a_plain_string() {
        let secret: Secret<String> = serde_json::from_str("\"hunter2\"").unwrap();
        assert_eq!(secret.expose_secret(), "hunter2");
    }
}
