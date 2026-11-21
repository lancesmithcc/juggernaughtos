//! Confidence-weighted, time-decaying score per subject.
//!
//! `score = Σ confidence × severity_weight × 0.5^(age / half_life)`
//! Repeated weak signals (failed logins, 404 probes) add up; one strong signal
//! (a honeytoken read) crosses the ban line on its own.

use crate::{Action, Event, Severity};
use jugg_core::config::StewardConfig;
use std::collections::HashMap;

fn weight(s: Severity) -> f64 {
    match s {
        Severity::Info => 0.0,
        Severity::Low => 0.25,
        Severity::Medium => 0.5,
        Severity::High => 0.85,
        Severity::Critical => 1.2,
    }
}

#[derive(Default)]
struct Subject {
    score: f64,
    updated: i64,
    incident_open: bool,
    banned_until: i64,
}

pub struct Correlator {
    cfg: StewardConfig,
    subjects: HashMap<String, Subject>,
    /// Never ban these (the owner's admin IPs, health checkers, loopback).
    allow: Vec<String>,
}

impl Correlator {
    pub fn new(cfg: StewardConfig, allow: Vec<String>) -> Self {
        Self { cfg, subjects: HashMap::new(), allow }
    }

    fn decay(&self, s: &mut Subject, now: i64) {
        let dt = (now - s.updated).max(0) as f64;
        s.score *= 0.5f64.powf(dt / self.cfg.score_half_life_secs as f64);
        s.updated = now;
    }

    pub fn score(&mut self, subject: &str, now: i64) -> f64 {
        let half_life = self.cfg.score_half_life_secs as f64;
        match self.subjects.get_mut(subject) {
            Some(s) => {
                let dt = (now - s.updated).max(0) as f64;
                s.score * 0.5f64.powf(dt / half_life)
            }
            None => 0.0,
        }
    }

    pub fn ingest(&mut self, ev: &Event) -> Vec<Action> {
        if ev.subject.is_empty() || self.allow.iter().any(|a| a == &ev.subject) {
            return vec![];
        }
        let mut s = self.subjects.remove(&ev.subject).unwrap_or(Subject { updated: ev.at, ..Default::default() });
        self.decay(&mut s, ev.at);
        s.score += ev.confidence.clamp(0.0, 1.0) * weight(ev.severity);

        let mut actions = Vec::new();
        if s.score >= self.cfg.incident_threshold && !s.incident_open {
            s.incident_open = true;
            actions.push(Action::OpenIncident { subject: ev.subject.clone(), score: s.score });
            actions.push(Action::Snapshot { subject: ev.subject.clone() });
        }
        if s.score >= self.cfg.ban_threshold && s.banned_until <= ev.at && looks_like_ip(&ev.subject) {
            s.banned_until = ev.at + self.cfg.ban_secs as i64;
            actions.push(Action::Ban { subject: ev.subject.clone(), secs: self.cfg.ban_secs });
        }
        if s.score < self.cfg.incident_threshold / 4.0 {
            s.incident_open = false;
        }
        self.subjects.insert(ev.subject.clone(), s);
        actions
    }
}

fn looks_like_ip(s: &str) -> bool {
    s.parse::<std::net::IpAddr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(subject: &str, sev: Severity, conf: f64, at: i64) -> Event {
        Event { source: "http".into(), kind: "probe".into(), severity: sev, confidence: conf, subject: subject.into(), detail: json!({}), at }
    }

    #[test]
    fn weak_signals_accumulate_strong_signal_bans_immediately() {
        let mut c = Correlator::new(StewardConfig::default(), vec!["10.0.0.5".into()]);
        // 404 probes: low/0.3 each → 0.075 per hit. A few are noise…
        let mut banned = false;
        for i in 0..5 {
            banned |= c.ingest(&ev("203.0.113.9", Severity::Low, 0.3, 1000 + i)).iter().any(|a| matches!(a, Action::Ban { .. }));
        }
        assert!(!banned, "5 stray 404s must not ban");
        // …but a burst of scanning accumulates past the ban line (fail2ban-like).
        for i in 5..20 {
            banned |= c.ingest(&ev("203.0.113.9", Severity::Low, 0.3, 1000 + i)).iter().any(|a| matches!(a, Action::Ban { .. }));
        }
        assert!(banned, "a 20-request scan burst must ban");
        // Honeytoken read: critical/0.95 → 1.14 → ban on its own
        let acts = c.ingest(&ev("198.51.100.7", Severity::Critical, 0.95, 2000));
        assert!(acts.iter().any(|a| matches!(a, Action::Ban { .. })));
        assert!(acts.iter().any(|a| matches!(a, Action::OpenIncident { .. })));
        // Allow-listed subject is never acted on
        assert!(c.ingest(&ev("10.0.0.5", Severity::Critical, 1.0, 2000)).is_empty());
    }

    #[test]
    fn scores_decay() {
        let mut c = Correlator::new(StewardConfig::default(), vec![]);
        c.ingest(&ev("192.0.2.1", Severity::High, 0.8, 0));
        let now = c.score("192.0.2.1", 0);
        let later = c.score("192.0.2.1", 1800);
        assert!((later - now / 2.0).abs() < 1e-9, "one half-life halves the score");
    }
}
