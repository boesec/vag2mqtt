//! Cookies that survive a restart.
//!
//! The EU Data Act portal hands out a session cookie rather than a token, so the session has to
//! be stored like one. `reqwest` keeps cookies in memory inside its client, which is the wrong
//! lifetime and, worse, would be shared between accounts. So every response's `Set-Cookie` is
//! captured here as well, and a client is rebuilt from the captured set on the next call.
//!
//! Sending is still left to `reqwest`, which knows the domain and path rules. This type only
//! observes and restores, with one simplification: a cookie is filed under its `Domain`
//! attribute when it has one, otherwise under the host that sent it.

use std::collections::BTreeMap;

use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use url::Url;

/// A stored cookie jar, keyed by the domain it belongs to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Cookies {
    by_domain: BTreeMap<String, BTreeMap<String, String>>,
}

impl Cookies {
    /// `true` when nothing has been stored.
    pub(crate) fn is_empty(&self) -> bool {
        self.by_domain.values().all(BTreeMap::is_empty)
    }

    /// How many cookies are stored, for a log line that says something without saying what.
    pub(crate) fn len(&self) -> usize {
        self.by_domain.values().map(BTreeMap::len).sum()
    }

    /// Records every `Set-Cookie` of a response.
    pub(crate) fn absorb(&mut self, url: &Url, headers: &HeaderMap) {
        let Some(host) = url.host_str() else {
            return;
        };
        for value in headers.get_all(reqwest::header::SET_COOKIE) {
            let Ok(text) = value.to_str() else {
                continue;
            };
            let Some((pair, attributes)) = split_cookie(text) else {
                continue;
            };
            let Some((name, value)) = pair.split_once('=') else {
                continue;
            };
            let name = name.trim();
            if name.is_empty() {
                continue;
            }
            let domain = attribute(attributes, "domain")
                .map(|domain| domain.trim_start_matches('.').to_ascii_lowercase())
                .unwrap_or_else(|| host.to_ascii_lowercase());
            let jar = self.by_domain.entry(domain).or_default();
            // An expiry in the past is a deletion, which is how a service signs a session out.
            if is_expired(attributes) || value.is_empty() {
                jar.remove(name);
            } else {
                jar.insert(name.to_string(), value.to_string());
            }
        }
    }

    /// Every cookie as `name=value` together with the domain it belongs to, for seeding a client.
    pub(crate) fn as_pairs(&self) -> Vec<(String, String)> {
        self.by_domain
            .iter()
            .flat_map(|(domain, jar)| {
                jar.iter()
                    .map(move |(name, value)| (domain.clone(), format!("{name}={value}")))
            })
            .collect()
    }
}

/// Splits `name=value; Attr=x; Attr2` into the pair and the attributes.
fn split_cookie(header: &str) -> Option<(&str, &str)> {
    match header.split_once(';') {
        Some((pair, attributes)) => Some((pair.trim(), attributes)),
        None => Some((header.trim(), "")),
    }
}

/// The value of an attribute, matched without regard to case.
fn attribute<'a>(attributes: &'a str, name: &str) -> Option<&'a str> {
    attributes.split(';').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// `true` for `Max-Age=0` or a negative age, which is how a cookie is deleted.
fn is_expired(attributes: &str) -> bool {
    attribute(attributes, "max-age")
        .and_then(|value| value.trim().parse::<i64>().ok())
        .is_some_and(|age| age <= 0)
}

#[cfg(test)]
mod tests {
    use reqwest::header::{HeaderValue, SET_COOKIE};

    use super::*;

    fn headers(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(SET_COOKIE, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    fn url(text: &str) -> Url {
        Url::parse(text).unwrap()
    }

    #[test]
    fn cookies_are_filed_under_their_domain_or_the_sending_host() {
        let mut cookies = Cookies::default();
        cookies.absorb(
            &url("https://identity.vwgroup.io/oidc/v1/authorize"),
            &headers(&[
                "SESSION=abc; Path=/; HttpOnly",
                "shared=xyz; Domain=.vwgroup.io; Path=/",
            ]),
        );
        let pairs = cookies.as_pairs();
        assert!(pairs.contains(&("identity.vwgroup.io".into(), "SESSION=abc".into())));
        assert!(pairs.contains(&("vwgroup.io".into(), "shared=xyz".into())));
        assert_eq!(cookies.len(), 2);
        assert!(!cookies.is_empty());
    }

    #[test]
    fn a_later_value_replaces_an_earlier_one() {
        let mut cookies = Cookies::default();
        let target = url("https://portal.example.test/login");
        cookies.absorb(&target, &headers(&["SESSION=first"]));
        cookies.absorb(&target, &headers(&["SESSION=second"]));
        assert_eq!(
            cookies.as_pairs(),
            vec![(
                "portal.example.test".to_string(),
                "SESSION=second".to_string()
            )]
        );
    }

    #[test]
    fn a_deletion_removes_the_cookie() {
        let mut cookies = Cookies::default();
        let target = url("https://portal.example.test/login");
        cookies.absorb(&target, &headers(&["SESSION=value"]));
        cookies.absorb(&target, &headers(&["SESSION=; Max-Age=0"]));
        assert!(cookies.is_empty(), "{:?}", cookies.as_pairs());
    }

    #[test]
    fn the_jar_round_trips_through_serde() {
        let mut cookies = Cookies::default();
        cookies.absorb(
            &url("https://portal.example.test/login"),
            &headers(&["SESSION=value; Path=/"]),
        );
        let json = serde_json::to_string(&cookies).unwrap();
        let back: Cookies = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cookies);
    }

    #[test]
    fn malformed_headers_are_ignored_rather_than_fatal() {
        let mut cookies = Cookies::default();
        cookies.absorb(
            &url("https://portal.example.test/"),
            &headers(&["novalue", "=orphan", "good=yes"]),
        );
        assert_eq!(cookies.len(), 1);
        assert!(
            cookies
                .as_pairs()
                .contains(&("portal.example.test".into(), "good=yes".into()))
        );
    }
}
