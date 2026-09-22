//! Wall clock abstraction so tests can control "now".

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};

/// Source of the current wall clock time.
pub trait Clock: Send + Sync {
    /// The current time.
    fn now(&self) -> DateTime<Utc>;
}

/// The real clock.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock tests move by hand.
#[derive(Clone, Debug)]
pub struct ManualClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl ManualClock {
    /// A clock stopped at `start`.
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            now: Arc::new(Mutex::new(start)),
        }
    }

    /// Moves the clock forward.
    pub fn advance(&self, by: chrono::Duration) {
        let mut now = self.now.lock().unwrap_or_else(|p| p.into_inner());
        *now += by;
    }

    /// Sets the clock.
    pub fn set(&self, to: DateTime<Utc>) {
        let mut now = self.now.lock().unwrap_or_else(|p| p.into_inner());
        *now = to;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().unwrap_or_else(|p| p.into_inner())
    }
}
