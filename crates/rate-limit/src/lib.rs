//! Sliding-window per-user rate limiter.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct RateLimiter {
    max: usize,
    window: Duration,
    hits: Mutex<HashMap<String, Vec<Instant>>>,
}

impl RateLimiter {
    pub fn new(max: usize, window: Duration) -> Self {
        Self {
            max,
            window,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Record an attempt; return `true` when the user is now over the limit (attempt rejected).
    pub fn check(&self, user: &str) -> bool {
        self.check_at(user, Instant::now())
    }

    fn check_at(&self, user: &str, now: Instant) -> bool {
        let mut hits = self.hits.lock().unwrap();
        // Hits are pushed in order, so a user whose newest hit has expired has
        // no live hits left; dropping such users keeps the map from growing
        // without bound as one-off identities come and go.
        hits.retain(|_, times| {
            times
                .last()
                .is_some_and(|t| now.duration_since(*t) < self.window)
        });
        let entry = hits.entry(user.to_string()).or_default();
        entry.retain(|t| now.duration_since(*t) < self.window);
        if entry.len() >= self.max {
            return true;
        }
        entry.push(now);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_max() {
        let rl = RateLimiter::new(3, Duration::from_secs(600));
        assert!(!rl.check("u"));
        assert!(!rl.check("u"));
        assert!(!rl.check("u"));
        assert!(rl.check("u"));
    }

    #[test]
    fn is_per_user() {
        let rl = RateLimiter::new(1, Duration::from_secs(600));
        assert!(!rl.check("a"));
        assert!(!rl.check("b"));
        assert!(rl.check("a"));
    }

    #[test]
    fn forgets_old_hits() {
        let rl = RateLimiter::new(1, Duration::from_millis(0));
        assert!(!rl.check("u"));
        assert!(!rl.check("u"));
    }

    #[test]
    fn evicts_users_whose_hits_have_all_expired() {
        let rl = RateLimiter::new(3, Duration::from_secs(10));
        let t0 = Instant::now();
        assert!(!rl.check_at("stale", t0));
        assert!(!rl.check_at("other", t0 + Duration::from_secs(20)));
        let hits = rl.hits.lock().unwrap();
        assert!(!hits.contains_key("stale"));
        assert!(hits.contains_key("other"));
    }

    #[test]
    fn check_at_uses_provided_instant() {
        let rl = RateLimiter::new(2, Duration::from_secs(10));
        let t0 = Instant::now();
        assert!(!rl.check_at("u", t0));
        assert!(!rl.check_at("u", t0 + Duration::from_secs(1)));
        // Both hits still in window — 3rd is blocked.
        assert!(rl.check_at("u", t0 + Duration::from_secs(2)));
        // After window expires the slot opens again.
        assert!(!rl.check_at("u", t0 + Duration::from_secs(20)));
    }
}
