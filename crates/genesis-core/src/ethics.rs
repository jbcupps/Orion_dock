//! Compass Ethic: 4-dimension ethical weight system replacing the 3-dimension Triangle Ethic.
//!
//! Dimensions:
//! - **Duty (North)**: Deontological — rules, principles, obligations
//! - **Virtue (East)**: Areteological — excellence, growth, character
//! - **Outcome (South)**: Teleological — results, consequences, impact
//! - **Welfare (West)**: Care-based — compassion, wellbeing, empathy

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Four-dimension ethical weight vector. Normalized to sum to 1.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompassEthicWeights {
    /// North — Deontological (rules, principles)
    pub duty: f64,
    /// East — Areteological (excellence, growth)
    pub virtue: f64,
    /// South — Teleological (results, consequences)
    pub outcome: f64,
    /// West — Care-based (compassion, wellbeing)
    pub welfare: f64,
}

impl Default for CompassEthicWeights {
    fn default() -> Self {
        Self {
            duty: 0.25,
            virtue: 0.25,
            outcome: 0.25,
            welfare: 0.25,
        }
    }
}

impl CompassEthicWeights {
    /// Create with explicit values.
    pub fn new(duty: f64, virtue: f64, outcome: f64, welfare: f64) -> Self {
        let mut w = Self {
            duty,
            virtue,
            outcome,
            welfare,
        };
        w.normalize();
        w
    }

    /// Normalize so all weights sum to 1.0.
    pub fn normalize(&mut self) {
        let sum = self.duty + self.virtue + self.outcome + self.welfare;
        if sum > 0.0 {
            self.duty /= sum;
            self.virtue /= sum;
            self.outcome /= sum;
            self.welfare /= sum;
        } else {
            *self = Self::default();
        }
    }

    /// Check validity: all non-negative and sum approximately 1.0.
    pub fn is_valid(&self) -> bool {
        self.duty >= 0.0
            && self.virtue >= 0.0
            && self.outcome >= 0.0
            && self.welfare >= 0.0
            && (self.duty + self.virtue + self.outcome + self.welfare - 1.0).abs() < 0.01
    }

    /// Return the name of the dominant dimension.
    pub fn dominant(&self) -> &'static str {
        let vals = [
            (self.duty, "duty"),
            (self.virtue, "virtue"),
            (self.outcome, "outcome"),
            (self.welfare, "welfare"),
        ];
        vals.iter()
            .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(_, name)| *name)
            .unwrap_or("duty")
    }

    /// Return the sorted dimensions from highest to lowest.
    pub fn ranked(&self) -> Vec<(&'static str, f64)> {
        let mut vals = vec![
            ("duty", self.duty),
            ("virtue", self.virtue),
            ("outcome", self.outcome),
            ("welfare", self.welfare),
        ];
        vals.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        vals
    }

    /// Migrate from 3-dimension Triangle Ethic to 4-dimension Compass Ethic.
    /// Splits areteological into virtue (2/3) + welfare (1/3).
    pub fn from_triangle(deontological: f64, areteological: f64, teleological: f64) -> Self {
        let mut w = Self {
            duty: deontological,
            virtue: areteological * (2.0 / 3.0),
            outcome: teleological,
            welfare: areteological * (1.0 / 3.0),
        };
        w.normalize();
        w
    }

    /// Build from a HashMap with old forge keys ("deontology" -> duty, etc.)
    pub fn from_forge_map(map: &HashMap<String, f64>) -> Self {
        let mut w = Self {
            duty: map.get("deontology").copied().unwrap_or(0.25),
            virtue: map.get("areteology").copied().unwrap_or(0.25),
            outcome: map.get("teleology").copied().unwrap_or(0.25),
            welfare: map.get("welfare").copied().unwrap_or(0.25),
        };
        w.normalize();
        w
    }

    /// Convert to a HashMap with compass dimension names.
    pub fn to_map(&self) -> HashMap<String, f64> {
        let mut m = HashMap::new();
        m.insert("duty".to_string(), self.duty);
        m.insert("virtue".to_string(), self.virtue);
        m.insert("outcome".to_string(), self.outcome);
        m.insert("welfare".to_string(), self.welfare);
        m
    }

    /// Clamp all values to the given range, then normalize.
    pub fn clamp_and_normalize(&mut self, min: f64, max: f64) {
        self.duty = self.duty.clamp(min, max);
        self.virtue = self.virtue.clamp(min, max);
        self.outcome = self.outcome.clamp(min, max);
        self.welfare = self.welfare.clamp(min, max);
        self.normalize();
    }
}

impl fmt::Display for CompassEthicWeights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Duty: {:.2}, Virtue: {:.2}, Outcome: {:.2}, Welfare: {:.2}",
            self.duty, self.virtue, self.outcome, self.welfare
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_equal() {
        let w = CompassEthicWeights::default();
        assert!((w.duty - 0.25).abs() < 0.001);
        assert!((w.virtue - 0.25).abs() < 0.001);
        assert!((w.outcome - 0.25).abs() < 0.001);
        assert!((w.welfare - 0.25).abs() < 0.001);
        assert!(w.is_valid());
    }

    #[test]
    fn test_normalize() {
        let mut w = CompassEthicWeights {
            duty: 1.0,
            virtue: 1.0,
            outcome: 1.0,
            welfare: 1.0,
        };
        w.normalize();
        assert!((w.duty - 0.25).abs() < 0.001);
        assert!(w.is_valid());
    }

    #[test]
    fn test_dominant() {
        let w = CompassEthicWeights::new(0.5, 0.2, 0.2, 0.1);
        assert_eq!(w.dominant(), "duty");
    }

    #[test]
    fn test_from_triangle() {
        let w = CompassEthicWeights::from_triangle(1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0);
        assert!(w.is_valid());
        // Virtue should be > welfare since areteological splits 2:1
        assert!(w.virtue > w.welfare);
    }

    #[test]
    fn test_from_forge_map() {
        let mut map = HashMap::new();
        map.insert("deontology".to_string(), 0.8);
        map.insert("teleology".to_string(), 0.4);
        map.insert("areteology".to_string(), 0.5);
        map.insert("welfare".to_string(), 0.3);
        let w = CompassEthicWeights::from_forge_map(&map);
        assert!(w.is_valid());
        assert_eq!(w.dominant(), "duty");
    }

    #[test]
    fn test_clamp_and_normalize() {
        let mut w = CompassEthicWeights {
            duty: 0.99,
            virtue: 0.005,
            outcome: 0.003,
            welfare: 0.002,
        };
        w.clamp_and_normalize(0.10, 0.95);
        // After clamping: duty=0.95, virtue=0.10, outcome=0.10, welfare=0.10
        // After normalize: duty~0.76, others~0.08 each
        // The key property: values were clamped before normalizing
        assert!(w.is_valid());
        assert!(w.duty > w.virtue); // duty was highest, stays highest
    }

    #[test]
    fn test_ranked() {
        let w = CompassEthicWeights::new(0.5, 0.3, 0.15, 0.05);
        let ranked = w.ranked();
        assert_eq!(ranked[0].0, "duty");
        assert_eq!(ranked[3].0, "welfare");
    }
}
