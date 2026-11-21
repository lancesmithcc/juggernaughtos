//! Failure-based limiter for sign-in attempts. Counts *failures* per key
//! (IP, and per account) in a sliding window; successes reset the account key.
//! A 6-digit TOTP has 10^6 codes and accepts 3 per attempt, so capping an
//! account at 10 failures per 15 minutes bounds online guessing to ~1 in 33 000
//! per window, and the steward bans IPs that keep trying.

use std::collections::HashMap;

pub struct Limiter {
    max_failures: u32,
    window_secs: i64,
    hits: HashMap<String, (u32, i64)>,
}

impl Limiter {
    pub fn new(max_failures: u32, window_secs: i64) -> Self {
        Self { max_failures, window_secs, hits: HashMap::new() }
    }

    /// Default policy for sign-in: 10 failures / 15 minutes.
    pub fn sign_in() -> Self {
        Self::new(10, 15 * 60)
    }

    /// Is `key` currently blocked?
    pub fn blocked(&mut self, key: &str, now: i64) -> bool {
        match self.hits.get(key) {
            Some((n, start)) if now - start < self.window_secs => *n >= self.max_failures,
            Some(_) => {
                self.hits.remove(key);
                false
            }
            None => false,
        }
    }

    pub fn fail(&mut self, key: &str, now: i64) {
        let e = self.hits.entry(key.to_string()).or_insert((0, now));
        if now - e.1 >= self.window_secs {
            *e = (0, now);
        }
        e.0 += 1;
        if self.hits.len() > 50_000 {
            let w = self.window_secs;
            self.hits.retain(|_, (_, s)| now - *s < w);
        }
    }

    pub fn succeed(&mut self, key: &str) {
        self.hits.remove(key);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn blocks_after_max_and_recovers() {
        let mut l = super::Limiter::new(3, 100);
        for _ in 0..3 {
            assert!(!l.blocked("ip:1", 0));
            l.fail("ip:1", 0);
        }
        assert!(l.blocked("ip:1", 10));
        assert!(!l.blocked("ip:2", 10));
        assert!(!l.blocked("ip:1", 101), "window expired");
        l.fail("acct", 0);
        l.succeed("acct");
        assert!(!l.blocked("acct", 1));
    }
}
