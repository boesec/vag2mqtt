//! Exponential backoff with jitter.

use std::time::Duration;

use rand::Rng;

use crate::settings::RuntimeSettings;

/// The delay before retry number `attempt` (1 for the first retry): `initial × factor^(attempt-1)`,
/// capped at `backoff_max`, with symmetric jitter.
pub(crate) fn delay(settings: &RuntimeSettings, attempt: u32, rng: &mut impl Rng) -> Duration {
    let exponent = attempt.saturating_sub(1).min(30);
    let base =
        settings.backoff_initial.as_secs_f64() * settings.backoff_factor.powi(exponent as i32);
    let capped = base.min(settings.backoff_max.as_secs_f64());
    let jitter = settings.backoff_jitter.clamp(0.0, 1.0);
    let factor = if jitter > 0.0 {
        rng.random_range((1.0 - jitter)..=(1.0 + jitter))
    } else {
        1.0
    };
    Duration::from_secs_f64((capped * factor).max(0.0))
}

#[cfg(test)]
mod tests {
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use super::*;

    fn settings() -> RuntimeSettings {
        RuntimeSettings {
            backoff_initial: Duration::from_secs(30),
            backoff_max: Duration::from_secs(1800),
            backoff_factor: 2.0,
            backoff_jitter: 0.2,
            ..RuntimeSettings::default()
        }
    }

    #[test]
    fn doubles_from_the_initial_delay_within_jitter() {
        let settings = settings();
        let mut rng = StdRng::seed_from_u64(42);
        for (attempt, expected) in [(1, 30.0), (2, 60.0), (3, 120.0), (4, 240.0)] {
            let d = delay(&settings, attempt, &mut rng).as_secs_f64();
            assert!(
                (expected * 0.8..=expected * 1.2).contains(&d),
                "attempt {attempt}: {d} not within ±20 % of {expected}"
            );
        }
    }

    #[test]
    fn caps_at_the_maximum() {
        let settings = settings();
        let mut rng = StdRng::seed_from_u64(1);
        let d = delay(&settings, 20, &mut rng).as_secs_f64();
        assert!((1800.0 * 0.8..=1800.0 * 1.2).contains(&d), "{d}");
    }

    #[test]
    fn zero_jitter_is_exact() {
        let settings = RuntimeSettings {
            backoff_jitter: 0.0,
            ..settings()
        };
        let mut rng = StdRng::seed_from_u64(1);
        assert_eq!(delay(&settings, 3, &mut rng), Duration::from_secs(120));
    }
}
