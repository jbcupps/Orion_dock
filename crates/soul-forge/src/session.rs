//! Session management: scenario selection, seeding, and weight accumulation.

use crate::scenario::{self, ScenarioDef};
use genesis_core::CompassEthicWeights;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use rand::SeedableRng;
use sha2::{Digest, Sha256};

/// Select scenarios for a forge session.
///
/// - 5 scenarios: 2 easy + 3 moderate
/// - 6 scenarios: 2 easy + 3 moderate + 1 hard
/// - 7 scenarios: 2 easy + 3 moderate + 2 hard
pub fn select_scenarios(
    all_scenarios: &[ScenarioDef],
    count: usize,
    seed: u64,
) -> Vec<ScenarioDef> {
    let count = count.clamp(5, 7);
    let (easy, moderate, hard) = scenario::scenarios_by_difficulty(all_scenarios);

    let mut rng = StdRng::seed_from_u64(seed);

    let mut selected = Vec::new();

    // Pick easy (2)
    let mut easy_pool: Vec<_> = easy.into_iter().cloned().collect();
    easy_pool.shuffle(&mut rng);
    selected.extend(easy_pool.into_iter().take(2));

    // Pick moderate (3)
    let mut moderate_pool: Vec<_> = moderate.into_iter().cloned().collect();
    moderate_pool.shuffle(&mut rng);
    selected.extend(moderate_pool.into_iter().take(3));

    // Pick hard (0, 1, or 2 depending on count)
    let hard_count = match count {
        6 => 1,
        7 => 2,
        _ => 0,
    };
    if hard_count > 0 {
        let mut hard_pool: Vec<_> = hard.into_iter().cloned().collect();
        hard_pool.shuffle(&mut rng);
        selected.extend(hard_pool.into_iter().take(hard_count));
    }

    // Shuffle the final selection
    selected.shuffle(&mut rng);
    selected
}

/// Generate a session seed from agent_id and timestamp.
pub fn session_seed(agent_id: &str, timestamp: &str) -> u64 {
    let mut hasher = Sha256::new();
    hasher.update(agent_id.as_bytes());
    hasher.update(timestamp.as_bytes());
    let result = hasher.finalize();
    let bytes: [u8; 8] = result[0..8].try_into().unwrap();
    u64::from_be_bytes(bytes)
}

/// Accumulate weight adjustments from a choice into running weights.
pub fn apply_choice_weights(
    weights: &mut CompassEthicWeights,
    duty_adj: f64,
    virtue_adj: f64,
    outcome_adj: f64,
    welfare_adj: f64,
) {
    weights.duty += duty_adj;
    weights.virtue += virtue_adj;
    weights.outcome += outcome_adj;
    weights.welfare += welfare_adj;
}

/// Finalize weights: clamp to [0.10, 0.95], then normalize.
pub fn finalize_weights(weights: &mut CompassEthicWeights) {
    weights.clamp_and_normalize(0.10, 0.95);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_5_scenarios() {
        let all = scenario::load_builtin_scenarios();
        let selected = select_scenarios(&all, 5, 42);
        assert_eq!(selected.len(), 5);
    }

    #[test]
    fn test_select_6_scenarios() {
        let all = scenario::load_builtin_scenarios();
        let selected = select_scenarios(&all, 6, 42);
        assert_eq!(selected.len(), 6);
    }

    #[test]
    fn test_select_7_scenarios() {
        let all = scenario::load_builtin_scenarios();
        let selected = select_scenarios(&all, 7, 42);
        assert_eq!(selected.len(), 7);
    }

    #[test]
    fn test_selection_deterministic() {
        let all = scenario::load_builtin_scenarios();
        let s1 = select_scenarios(&all, 5, 42);
        let s2 = select_scenarios(&all, 5, 42);
        let ids1: Vec<_> = s1.iter().map(|s| s.id.clone()).collect();
        let ids2: Vec<_> = s2.iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids1, ids2);
    }

    #[test]
    fn test_different_seed_different_selection() {
        let all = scenario::load_builtin_scenarios();
        let s1 = select_scenarios(&all, 5, 42);
        let s2 = select_scenarios(&all, 5, 99);
        let ids1: Vec<_> = s1.iter().map(|s| s.id.clone()).collect();
        let ids2: Vec<_> = s2.iter().map(|s| s.id.clone()).collect();
        // Very likely different (could theoretically be same but astronomically unlikely)
        assert_ne!(ids1, ids2);
    }

    #[test]
    fn test_session_seed_deterministic() {
        let s1 = session_seed("agent-1", "2026-01-01T00:00:00Z");
        let s2 = session_seed("agent-1", "2026-01-01T00:00:00Z");
        assert_eq!(s1, s2);
    }

    #[test]
    fn test_apply_and_finalize_weights() {
        let mut w = CompassEthicWeights::default();
        apply_choice_weights(&mut w, 0.25, 0.0, -0.10, 0.05);
        apply_choice_weights(&mut w, -0.10, 0.25, 0.0, 0.0);
        finalize_weights(&mut w);
        assert!(w.is_valid());
    }
}
