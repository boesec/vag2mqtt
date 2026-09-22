//! The one place that assembles topic strings.
//!
//! Every topic of the contract is a method here. No other file in this crate joins strings
//! with `/`; a unit test enforces that.

use vag2mqtt_domain::{AccountId, Vin};

use crate::error::MqttError;

/// Builds every topic of the contract from one prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopicBuilder {
    prefix: String,
}

impl TopicBuilder {
    /// Validates the prefix: non-empty, no wildcards, no leading or trailing `/`, no control
    /// characters.
    pub fn new(prefix: impl Into<String>) -> Result<Self, MqttError> {
        let prefix = prefix.into();
        if prefix.is_empty() {
            return Err(MqttError::InvalidPrefix { reason: "empty" });
        }
        if prefix.starts_with('/') || prefix.ends_with('/') {
            return Err(MqttError::InvalidPrefix {
                reason: "must not start or end with '/'",
            });
        }
        validate_segment_chars(&prefix).map_err(|reason| MqttError::InvalidPrefix { reason })?;
        Ok(Self { prefix })
    }

    /// The prefix.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    fn join(&self, segments: &[&str]) -> String {
        let mut topic = String::with_capacity(self.prefix.len() + segments.len() * 12);
        topic.push_str(&self.prefix);
        for segment in segments {
            topic.push('/');
            topic.push_str(segment);
        }
        topic
    }

    /// `<prefix>/connected`
    pub fn connected(&self) -> String {
        self.join(&["connected"])
    }

    /// `<prefix>/info`
    pub fn info(&self) -> String {
        self.join(&["info"])
    }

    /// `<prefix>/status/<VIN>/<path>`
    pub fn status(&self, vin: &Vin, path: &str) -> String {
        self.join(&["status", vin.as_str(), path])
    }

    /// `<prefix>/status/<VIN>/full`
    pub fn full(&self, vin: &Vin) -> String {
        self.status(vin, "full")
    }

    /// `<prefix>/status/<VIN>/availability`
    pub fn vehicle_availability(&self, vin: &Vin) -> String {
        self.status(vin, "availability")
    }

    /// `<prefix>/status/<VIN>/` (the prefix of every topic of that vehicle)
    pub fn vehicle_status_prefix(&self, vin: &Vin) -> String {
        let mut prefix = self.join(&["status", vin.as_str()]);
        prefix.push('/');
        prefix
    }

    /// `<prefix>/status/account/<id>/availability`
    pub fn account_availability(&self, id: &AccountId) -> String {
        self.join(&["status", "account", id.as_str(), "availability"])
    }

    /// `<prefix>/maintenance/stats`
    pub fn maintenance_stats(&self) -> String {
        self.join(&["maintenance", "stats"])
    }

    /// `<prefix>/maintenance/set/loglevel`
    pub fn maintenance_set_loglevel(&self) -> String {
        self.join(&["maintenance", "set", "loglevel"])
    }

    /// `<prefix>/set/#`, the subscription for vehicle commands.
    pub fn set_wildcard(&self) -> String {
        self.join(&["set", "#"])
    }

    /// `<prefix>/maintenance/set/#`, the subscription for maintenance commands.
    pub fn maintenance_set_wildcard(&self) -> String {
        self.join(&["maintenance", "set", "#"])
    }

    /// Splits an incoming `<prefix>/set/<VIN>/<command>` topic into VIN and command.
    pub fn parse_set(&self, topic: &str) -> Option<(String, String)> {
        let rest = topic.strip_prefix(&self.join(&["set", ""]))?;
        let (vin, command) = rest.split_once('/')?;
        if vin.is_empty() || command.is_empty() {
            return None;
        }
        Some((vin.to_string(), command.to_string()))
    }
}

/// Validates the characters of a segment or prefix the contract puts into a topic.
pub(crate) fn validate_segment_chars(segment: &str) -> Result<(), &'static str> {
    if segment.contains(['+', '#']) {
        return Err("must not contain the wildcards '+' or '#'");
    }
    if segment.contains('\0') {
        return Err("must not contain NUL");
    }
    if segment.chars().any(char::is_whitespace) {
        return Err("must not contain whitespace");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topics() -> TopicBuilder {
        TopicBuilder::new("vag2mqtt").unwrap()
    }

    fn vin() -> Vin {
        Vin::new("WAUZZZ0000000TEST").unwrap()
    }

    #[test]
    fn prefixes_are_validated() {
        assert!(TopicBuilder::new("").is_err());
        assert!(TopicBuilder::new("/vag2mqtt").is_err());
        assert!(TopicBuilder::new("vag2mqtt/").is_err());
        assert!(TopicBuilder::new("vag+2mqtt").is_err());
        assert!(TopicBuilder::new("vag2mqtt/#").is_err());
        assert!(TopicBuilder::new("vag 2mqtt").is_err());
        assert!(TopicBuilder::new("home/cars").is_ok());
    }

    #[test]
    fn every_topic_of_the_contract() {
        let t = topics();
        let id = AccountId::new("acc-1").unwrap();
        assert_eq!(t.connected(), "vag2mqtt/connected");
        assert_eq!(t.info(), "vag2mqtt/info");
        assert_eq!(
            t.status(&vin(), "battery/soc"),
            "vag2mqtt/status/WAUZZZ0000000TEST/battery/soc"
        );
        assert_eq!(t.full(&vin()), "vag2mqtt/status/WAUZZZ0000000TEST/full");
        assert_eq!(
            t.vehicle_availability(&vin()),
            "vag2mqtt/status/WAUZZZ0000000TEST/availability"
        );
        assert_eq!(
            t.vehicle_status_prefix(&vin()),
            "vag2mqtt/status/WAUZZZ0000000TEST/"
        );
        assert_eq!(
            t.account_availability(&id),
            "vag2mqtt/status/account/acc-1/availability"
        );
        assert_eq!(t.maintenance_stats(), "vag2mqtt/maintenance/stats");
        assert_eq!(
            t.maintenance_set_loglevel(),
            "vag2mqtt/maintenance/set/loglevel"
        );
        assert_eq!(t.set_wildcard(), "vag2mqtt/set/#");
        assert_eq!(t.maintenance_set_wildcard(), "vag2mqtt/maintenance/set/#");
    }

    #[test]
    fn set_topics_are_parsed() {
        let t = topics();
        assert_eq!(
            t.parse_set("vag2mqtt/set/WAUZZZ0000000TEST/climatisation"),
            Some(("WAUZZZ0000000TEST".to_string(), "climatisation".to_string()))
        );
        assert_eq!(t.parse_set("vag2mqtt/set/WAUZZZ0000000TEST"), None);
        assert_eq!(t.parse_set("other/set/x/y"), None);
        assert_eq!(t.parse_set("vag2mqtt/set//y"), None);
    }

    #[test]
    fn only_this_file_names_topic_level_segments() {
        // Contract paths (`battery/soc`) live in flatten.rs; the topic *structure* (`status`,
        // `connected`, `set`, ...) may only be spelled out here.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let forbidden = [
            "\"connected\"",
            "\"info\"",
            "\"status\"",
            "\"set\"",
            "\"maintenance\"",
            "/status/",
            "/set/",
            "/connected",
        ];
        for entry in std::fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().and_then(|n| n.to_str()) == Some("topic.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            for (number, line) in text.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.starts_with("#[cfg(test)]") {
                    break;
                }
                if trimmed.starts_with("//") {
                    continue;
                }
                for needle in forbidden {
                    assert!(
                        !trimmed.contains(needle),
                        "{}:{} spells out a topic segment outside topic.rs: {line}",
                        path.display(),
                        number + 1
                    );
                }
            }
        }
    }
}
