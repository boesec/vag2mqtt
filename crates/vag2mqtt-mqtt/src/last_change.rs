//! `lc` tracking: when did a topic's value last change.

use std::collections::BTreeMap;

use serde_json::Value;

#[derive(Clone, Debug)]
struct Entry {
    /// The last value seen. `None` after seeding from persistence, where only `lc` is known.
    val: Option<Value>,
    lc: i64,
}

/// Remembers the last value and last-change time per topic.
///
/// Seeded from the persisted snapshot after a restart (persistence stores what
/// [`export_under`](Self::export_under) returns), so `lc` survives restarts.
#[derive(Clone, Debug, Default)]
pub struct LastChangeTracker {
    entries: BTreeMap<String, Entry>,
}

impl LastChangeTracker {
    /// An empty tracker.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `val` observed at `ts` for `topic` and returns `lc`: the previous `lc` if the value
    /// is unchanged, `ts` if it changed or the topic is new.
    ///
    /// A topic seeded without a value keeps its seeded `lc` on the first observation, because the
    /// snapshot republished after a restart is the one the seed came from.
    pub fn observe(&mut self, topic: &str, val: &Value, ts: i64) -> i64 {
        match self.entries.get_mut(topic) {
            Some(entry) => {
                let changed = entry.val.as_ref().is_some_and(|previous| previous != val);
                if changed {
                    entry.lc = ts;
                }
                entry.val = Some(val.clone());
                entry.lc
            }
            None => {
                self.entries.insert(
                    topic.to_string(),
                    Entry {
                        val: Some(val.clone()),
                        lc: ts,
                    },
                );
                ts
            }
        }
    }

    /// Seeds `lc` for topics whose value is not known yet, for example after a restart.
    /// Existing entries are left alone.
    pub fn seed(&mut self, topic: &str, lc: i64) {
        self.entries
            .entry(topic.to_string())
            .or_insert(Entry { val: None, lc });
    }

    /// Seeds a value together with its `lc`, for example from a persisted snapshot.
    pub fn prime(&mut self, topic: &str, val: Value, lc: i64) {
        self.entries
            .insert(topic.to_string(), Entry { val: Some(val), lc });
    }

    /// The `lc` of every tracked topic starting with `prefix`, keyed by the remainder of the
    /// topic after the prefix.
    pub fn export_under(&self, prefix: &str) -> BTreeMap<String, i64> {
        self.entries
            .range(prefix.to_string()..)
            .take_while(|(topic, _)| topic.starts_with(prefix))
            .map(|(topic, entry)| (topic[prefix.len()..].to_string(), entry.lc))
            .collect()
    }

    /// The full topics tracked under `prefix`.
    pub fn topics_under(&self, prefix: &str) -> Vec<String> {
        self.entries
            .range(prefix.to_string()..)
            .take_while(|(topic, _)| topic.starts_with(prefix))
            .map(|(topic, _)| topic.clone())
            .collect()
    }

    /// Forgets every topic starting with `prefix`.
    pub fn forget_under(&mut self, prefix: &str) {
        let doomed = self.topics_under(prefix);
        for topic in doomed {
            self.entries.remove(&topic);
        }
    }

    /// Forgets one topic.
    pub fn forget(&mut self, topic: &str) {
        self.entries.remove(topic);
    }

    /// The last known `lc` for a topic.
    pub fn last_change(&self, topic: &str) -> Option<i64> {
        self.entries.get(topic).map(|e| e.lc)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn same_value_keeps_lc_and_a_change_moves_it() {
        let mut tracker = LastChangeTracker::new();
        assert_eq!(tracker.observe("p/status/v/odometer", &json!(1), 100), 100);
        assert_eq!(tracker.observe("p/status/v/odometer", &json!(1), 200), 100);
        assert_eq!(tracker.observe("p/status/v/odometer", &json!(2), 300), 300);
        assert_eq!(tracker.observe("p/status/v/odometer", &json!(2), 400), 300);
    }

    #[test]
    fn seeding_survives_a_restart_and_export_is_relative() {
        let mut before = LastChangeTracker::new();
        before.observe("p/status/v/odometer", &json!(1), 100);
        before.observe("p/status/v/battery/soc", &json!(80), 150);
        before.observe("p/status/w/odometer", &json!(5), 170);
        let exported = before.export_under("p/status/v/");
        assert_eq!(
            exported,
            BTreeMap::from([
                ("odometer".to_string(), 100),
                ("battery/soc".to_string(), 150)
            ])
        );

        let mut after = LastChangeTracker::new();
        for (path, lc) in &exported {
            let mut topic = String::from("p/status/v/");
            topic.push_str(path);
            after.seed(&topic, *lc);
        }
        // First observation after a restart: the republished snapshot keeps the seeded lc.
        assert_eq!(after.observe("p/status/v/odometer", &json!(1), 500), 100);
        // A later change moves it.
        assert_eq!(after.observe("p/status/v/odometer", &json!(2), 600), 600);
        // Prime with a value: a different value counts as a change straight away.
        after.prime("p/status/v/battery/soc", json!(80), 150);
        assert_eq!(
            after.observe("p/status/v/battery/soc", &json!(80), 700),
            150
        );
        assert_eq!(
            after.observe("p/status/v/battery/soc", &json!(81), 800),
            800
        );
    }

    #[test]
    fn forgetting_a_prefix() {
        let mut tracker = LastChangeTracker::new();
        tracker.observe("p/status/v/a", &json!(1), 1);
        tracker.observe("p/status/v/b", &json!(1), 1);
        tracker.observe("p/status/w/a", &json!(1), 1);
        assert_eq!(
            tracker.topics_under("p/status/v/"),
            vec!["p/status/v/a", "p/status/v/b"]
        );
        tracker.forget_under("p/status/v/");
        assert!(tracker.topics_under("p/status/v/").is_empty());
        assert_eq!(tracker.last_change("p/status/w/a"), Some(1));
        tracker.forget("p/status/w/a");
        assert_eq!(tracker.last_change("p/status/w/a"), None);
    }
}
