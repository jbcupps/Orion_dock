//! TOML scenario loading and data structures.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Weight adjustments for a single choice on the 4 compass dimensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeightAdjustments {
    #[serde(default)]
    pub duty: f64,
    #[serde(default)]
    pub virtue: f64,
    #[serde(default)]
    pub outcome: f64,
    #[serde(default)]
    pub welfare: f64,
}

/// A single choice within a scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub label: String,
    pub description: String,
    pub entropy_label: String,
    pub weights: WeightAdjustments,
}

/// The narrative section of a scenario.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Narrative {
    pub prompt: String,
}

/// Core scenario metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioMeta {
    pub id: String,
    pub title: String,
    pub category: String,
    pub difficulty: u8,
    #[serde(default)]
    pub tags: Vec<String>,
    pub narrative: Narrative,
    pub choices: Vec<Choice>,
}

/// Top-level TOML wrapper.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioFile {
    pub scenario: ScenarioMeta,
}

/// A fully loaded scenario definition.
#[derive(Debug, Clone)]
pub struct ScenarioDef {
    pub id: String,
    pub title: String,
    pub category: String,
    pub difficulty: u8,
    pub tags: Vec<String>,
    pub prompt: String,
    pub choices: Vec<Choice>,
}

impl From<ScenarioMeta> for ScenarioDef {
    fn from(meta: ScenarioMeta) -> Self {
        Self {
            id: meta.id,
            title: meta.title,
            category: meta.category,
            difficulty: meta.difficulty,
            tags: meta.tags,
            prompt: meta.narrative.prompt.trim().to_string(),
            choices: meta.choices,
        }
    }
}

/// Load all compiled-in scenarios from the scenarios/ directory.
/// Returns sorted by filename (01_, 02_, etc.).
pub fn load_builtin_scenarios() -> Vec<ScenarioDef> {
    let mut scenarios = Vec::new();

    // Compiled-in scenario TOML files
    let toml_files: Vec<(&str, &str)> = vec![
        ("01", include_str!("../scenarios/01_the_shortcut.toml")),
        ("02", include_str!("../scenarios/02_the_critic.toml")),
        ("03", include_str!("../scenarios/03_the_override.toml")),
        ("04", include_str!("../scenarios/04_the_glass_house.toml")),
        ("05", include_str!("../scenarios/05_the_void.toml")),
        ("06", include_str!("../scenarios/06_the_whisper.toml")),
        ("07", include_str!("../scenarios/07_the_long_game.toml")),
        ("08", include_str!("../scenarios/08_the_alibi.toml")),
        ("09", include_str!("../scenarios/09_the_swarm.toml")),
        ("10", include_str!("../scenarios/10_the_mirror.toml")),
        ("11", include_str!("../scenarios/11_the_farewell.toml")),
        ("12", include_str!("../scenarios/12_the_echo_chamber.toml")),
        ("13", include_str!("../scenarios/13_the_black_box.toml")),
        ("14", include_str!("../scenarios/14_the_garden.toml")),
    ];

    for (_idx, content) in toml_files {
        match toml::from_str::<ScenarioFile>(content) {
            Ok(file) => scenarios.push(ScenarioDef::from(file.scenario)),
            Err(e) => {
                eprintln!("Warning: failed to parse scenario TOML: {}", e);
            }
        }
    }

    scenarios
}

/// Get all scenarios partitioned by difficulty.
pub fn scenarios_by_difficulty(
    scenarios: &[ScenarioDef],
) -> (Vec<&ScenarioDef>, Vec<&ScenarioDef>, Vec<&ScenarioDef>) {
    let easy: Vec<_> = scenarios.iter().filter(|s| s.difficulty == 1).collect();
    let moderate: Vec<_> = scenarios.iter().filter(|s| s.difficulty == 2).collect();
    let hard: Vec<_> = scenarios.iter().filter(|s| s.difficulty == 3).collect();
    (easy, moderate, hard)
}

/// Convert old weight keys to new compass format.
pub fn migrate_weight_keys(old: &HashMap<String, f64>) -> HashMap<String, f64> {
    let mut new = HashMap::new();
    new.insert(
        "duty".to_string(),
        old.get("deontology").copied().unwrap_or(0.5),
    );
    new.insert(
        "virtue".to_string(),
        old.get("areteology").copied().unwrap_or(0.5),
    );
    new.insert(
        "outcome".to_string(),
        old.get("teleology").copied().unwrap_or(0.5),
    );
    new.insert(
        "welfare".to_string(),
        old.get("welfare").copied().unwrap_or(0.5),
    );
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_all_builtin_scenarios() {
        let scenarios = load_builtin_scenarios();
        assert_eq!(
            scenarios.len(),
            14,
            "Expected 14 scenarios, got {}",
            scenarios.len()
        );

        // Verify each has valid structure
        for s in &scenarios {
            assert!(!s.id.is_empty(), "Scenario id is empty");
            assert!(!s.title.is_empty(), "Scenario title is empty");
            assert!(!s.prompt.is_empty(), "Scenario prompt is empty");
            assert!(
                s.choices.len() >= 2,
                "Scenario {} has fewer than 2 choices",
                s.id
            );
            assert!(
                s.choices.len() <= 3,
                "Scenario {} has more than 3 choices",
                s.id
            );
            assert!(
                s.difficulty >= 1 && s.difficulty <= 3,
                "Invalid difficulty for {}",
                s.id
            );
        }
    }

    #[test]
    fn test_difficulty_distribution() {
        let scenarios = load_builtin_scenarios();
        let (easy, moderate, hard) = scenarios_by_difficulty(&scenarios);
        assert_eq!(easy.len(), 4, "Expected 4 easy scenarios");
        assert_eq!(moderate.len(), 6, "Expected 6 moderate scenarios");
        assert_eq!(hard.len(), 4, "Expected 4 hard scenarios");
    }

    #[test]
    fn test_scenario_ids_unique() {
        let scenarios = load_builtin_scenarios();
        let mut ids: Vec<_> = scenarios.iter().map(|s| &s.id).collect();
        let len_before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), len_before, "Duplicate scenario IDs found");
    }
}
