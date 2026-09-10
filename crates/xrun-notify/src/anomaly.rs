#![deny(unsafe_code)]

//! Cheap, stateful checks over the metric stream. Runs in the poll loop on
//! every batch, so everything here is O(1) per point and allocation-free
//! after warm-up.
//!
//! Two rules, both firing at most once per key per run:
//! - **non-finite**: any NaN / ±inf value on any key.
//! - **loss spike**: a key whose name contains `loss` jumps above
//!   `SPIKE_FACTOR × running-min` after at least `MIN_POINTS` points. That's
//!   the "learning rate too high / divergence" shape; a slow creep is not
//!   flagged (that's what the metrics chart is for).

use std::collections::{HashMap, HashSet};

pub const MIN_POINTS: usize = 10;
pub const SPIKE_FACTOR: f64 = 10.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Anomaly {
    NonFinite {
        key: String,
        step: Option<i64>,
        value: f64,
    },
    LossSpike {
        key: String,
        step: Option<i64>,
        value: f64,
        running_min: f64,
    },
}

impl Anomaly {
    pub fn key(&self) -> &str {
        match self {
            Anomaly::NonFinite { key, .. } | Anomaly::LossSpike { key, .. } => key,
        }
    }

    pub fn describe(&self) -> String {
        let at = |s: &Option<i64>| s.map(|v| format!(" at step {v}")).unwrap_or_default();
        match self {
            Anomaly::NonFinite { step, value, .. } => {
                format!("value is {value}{}", at(step))
            }
            Anomaly::LossSpike {
                step,
                value,
                running_min,
                ..
            } => format!(
                "{value:.4}{} vs running min {running_min:.4} (>{}x) — divergence?",
                at(step),
                SPIKE_FACTOR as u32
            ),
        }
    }
}

#[derive(Debug, Default)]
struct KeyState {
    count: usize,
    min: f64,
}

#[derive(Debug, Default)]
pub struct AnomalyDetector {
    keys: HashMap<String, KeyState>,
    fired: HashSet<String>,
}

fn is_loss_key(key: &str) -> bool {
    key.to_ascii_lowercase().contains("loss")
}

impl AnomalyDetector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one point. Returns an anomaly the first time a key trips a
    /// rule; later points on that key are tracked but never re-reported.
    pub fn observe(&mut self, key: &str, step: Option<i64>, value: f64) -> Option<Anomaly> {
        if self.fired.contains(key) {
            return None;
        }
        if !value.is_finite() {
            self.fired.insert(key.to_string());
            return Some(Anomaly::NonFinite {
                key: key.to_string(),
                step,
                value,
            });
        }
        if !is_loss_key(key) {
            return None;
        }
        let st = self
            .keys
            .entry(key.to_string())
            .or_insert_with(|| KeyState {
                count: 0,
                min: f64::INFINITY,
            });
        let prior_count = st.count;
        let prior_min = st.min;
        st.count += 1;
        if value < st.min {
            st.min = value;
        }
        if prior_count >= MIN_POINTS && prior_min > 0.0 && value > SPIKE_FACTOR * prior_min {
            self.fired.insert(key.to_string());
            return Some(Anomaly::LossSpike {
                key: key.to_string(),
                step,
                value,
                running_min: prior_min,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nan_fires_once_on_any_key() {
        let mut d = AnomalyDetector::new();
        assert!(matches!(
            d.observe("val_f1", Some(1), f64::NAN),
            Some(Anomaly::NonFinite { .. })
        ));
        assert_eq!(d.observe("val_f1", Some(2), f64::NAN), None);
        assert!(matches!(
            d.observe("train_loss", None, f64::INFINITY),
            Some(Anomaly::NonFinite { .. })
        ));
    }

    #[test]
    fn spike_needs_warmup_and_loss_key() {
        let mut d = AnomalyDetector::new();
        // 9 points then a spike: not enough history.
        for i in 0..9 {
            assert_eq!(d.observe("train_loss", Some(i), 1.0), None);
        }
        assert_eq!(d.observe("train_loss", Some(9), 100.0), None);
        // Now warmed up (10 points seen); a spike relative to min=1.0 fires.
        let a = d.observe("train_loss", Some(10), 50.0);
        assert!(matches!(a, Some(Anomaly::LossSpike { running_min, .. }) if running_min == 1.0));
        // Only once.
        assert_eq!(d.observe("train_loss", Some(11), 500.0), None);
        // Non-loss keys never spike.
        let mut d = AnomalyDetector::new();
        for i in 0..20 {
            assert_eq!(d.observe("lr", Some(i), 1.0), None);
        }
        assert_eq!(d.observe("lr", Some(20), 1000.0), None);
    }

    #[test]
    fn spike_ignores_nonpositive_min() {
        let mut d = AnomalyDetector::new();
        for i in 0..12 {
            d.observe("loss", Some(i), 0.0);
        }
        assert_eq!(d.observe("loss", Some(12), 5.0), None);
    }

    #[test]
    fn describe_is_human() {
        let a = Anomaly::LossSpike {
            key: "loss".into(),
            step: Some(42),
            value: 12.0,
            running_min: 0.5,
        };
        let s = a.describe();
        assert!(s.contains("step 42"));
        assert!(s.contains("divergence"));
    }
}
