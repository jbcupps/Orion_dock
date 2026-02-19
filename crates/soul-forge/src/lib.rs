//! Soul-forge library: calibration state machine and deterministic soul generation.
//! TUI entrypoint and rendering remain in `main.rs`.
//!
//! **v2**: Dynamic TOML-driven scenarios, 16-archetype combinatorial matrix,
//! Compass Ethic (4 dimensions), improved sigil generation.
//!
//! Genesis path contract: use `soul_output(name, purpose, personality)` to get soul data
//! without calling the birth orchestrator; the caller then calls `crystallize_soul`.

pub mod archetype;
pub mod scenario;
pub mod session;
pub mod sigil;

use anyhow::Result;
use genesis_core::CompassEthicWeights;
use scenario::ScenarioDef;
use std::collections::HashMap;
use std::fs;

/// Dynamic app state: Boot, Intro, Scenario(index), Crystallize, Done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Boot,
    Intro,
    /// Dynamic scenario index (0-based into selected_scenarios).
    Scenario(usize),
    Crystallize,
    Done,
}

/// Output of Soul Forge for the Genesis path contract. Returned by `soul_output()`;
/// caller uses `soul_content` and `growth_content` with `BirthOrchestrator::crystallize_soul`,
/// and may write `soul_json` to docs_dir/soul.json.
#[derive(Debug, Clone)]
pub struct SoulOutput {
    pub soul_content: String,
    pub growth_content: String,
    pub archetype: String,
    pub weights: HashMap<String, f64>,
    pub soul_hash: String,
    pub sigil_art: String,
    pub soul_json: serde_json::Value,
}

/// Application state and calibration logic (testable without terminal).
pub struct App {
    pub state: AppState,
    /// Compass ethic weights (4-dimension).
    pub compass_weights: CompassEthicWeights,
    /// Legacy weight map (for backward-compatible serialization).
    pub weights: HashMap<String, f64>,
    pub entropy_source: Vec<String>,
    pub boot_progress: u16,
    pub boot_logs: Vec<String>,
    pub list_state_selected: Option<usize>,
    pub archetype: String,
    pub archetype_detail: Option<archetype::ArchetypeResult>,
    pub sigil_art: String,
    pub soul_hash: String,
    /// Selected scenarios for this session.
    pub selected_scenarios: Vec<ScenarioDef>,
    /// Current scenario index within selected_scenarios.
    pub current_scenario_index: usize,
    /// Session seed for deterministic selection.
    pub session_seed: u64,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self::with_config(5, 0)
    }

    /// Create with specific scenario count and session seed.
    pub fn with_config(scenario_count: usize, seed: u64) -> Self {
        let all_scenarios = scenario::load_builtin_scenarios();
        let selected = session::select_scenarios(&all_scenarios, scenario_count, seed);

        Self {
            state: AppState::Boot,
            compass_weights: CompassEthicWeights::default(),
            weights: Self::compass_to_legacy(&CompassEthicWeights::default()),
            entropy_source: Vec::new(),
            boot_progress: 0,
            boot_logs: Vec::new(),
            list_state_selected: Some(0),
            archetype: String::new(),
            archetype_detail: None,
            sigil_art: String::new(),
            soul_hash: String::new(),
            selected_scenarios: selected,
            current_scenario_index: 0,
            session_seed: seed,
        }
    }

    /// Total number of scenarios in this session.
    pub fn scenario_count(&self) -> usize {
        self.selected_scenarios.len()
    }

    /// Get the current scenario definition (if in a Scenario state).
    pub fn current_scenario(&self) -> Option<&ScenarioDef> {
        if let AppState::Scenario(idx) = self.state {
            self.selected_scenarios.get(idx)
        } else {
            None
        }
    }

    /// Convert CompassEthicWeights to legacy HashMap for backward compat.
    fn compass_to_legacy(w: &CompassEthicWeights) -> HashMap<String, f64> {
        let mut m = HashMap::new();
        m.insert("duty".to_string(), w.duty);
        m.insert("virtue".to_string(), w.virtue);
        m.insert("outcome".to_string(), w.outcome);
        m.insert("welfare".to_string(), w.welfare);
        m
    }

    /// Sync legacy weights from compass weights.
    fn sync_legacy_weights(&mut self) {
        self.weights = Self::compass_to_legacy(&self.compass_weights);
    }

    pub fn next_stage(&mut self) {
        match self.state {
            AppState::Boot => self.state = AppState::Intro,
            AppState::Intro => {
                self.state = AppState::Scenario(0);
                self.current_scenario_index = 0;
                self.list_state_selected = Some(0);
            }
            AppState::Scenario(idx) => {
                let next = idx + 1;
                if next < self.selected_scenarios.len() {
                    self.state = AppState::Scenario(next);
                    self.current_scenario_index = next;
                    self.list_state_selected = Some(0);
                } else {
                    self.crystallize();
                    self.state = AppState::Crystallize;
                }
            }
            AppState::Crystallize => {
                self.state = AppState::Done;
            }
            AppState::Done => {}
        }
    }

    pub fn handle_selection(&mut self) {
        let selected = self.list_state_selected.unwrap_or(0);

        if let AppState::Scenario(idx) = self.state {
            // Extract values before mutating self
            let adjustment = self.selected_scenarios.get(idx).and_then(|scenario| {
                scenario.choices.get(selected).map(|choice| {
                    (
                        choice.weights.duty,
                        choice.weights.virtue,
                        choice.weights.outcome,
                        choice.weights.welfare,
                        choice.entropy_label.clone(),
                    )
                })
            });

            if let Some((duty, virtue, outcome, welfare, label)) = adjustment {
                session::apply_choice_weights(
                    &mut self.compass_weights,
                    duty,
                    virtue,
                    outcome,
                    welfare,
                );
                self.sync_legacy_weights();
                self.entropy_source.push(label);
            }
            self.next_stage();
        }
    }

    pub fn determine_archetype(&self) -> String {
        let result = archetype::determine_archetype(&self.compass_weights);
        result.name
    }

    pub fn generate_sigil(&mut self) {
        let scenario_ids: Vec<String> = self
            .selected_scenarios
            .iter()
            .map(|s| s.id.clone())
            .collect();

        let (hash, art) = sigil::generate_sigil(
            self.compass_weights.duty,
            self.compass_weights.virtue,
            self.compass_weights.outcome,
            self.compass_weights.welfare,
            &self.entropy_source,
            self.session_seed,
            &scenario_ids,
        );
        self.soul_hash = hash;
        self.sigil_art = art;
    }

    pub fn crystallize(&mut self) {
        // Finalize weights (clamp + normalize)
        session::finalize_weights(&mut self.compass_weights);
        self.sync_legacy_weights();

        let result = archetype::determine_archetype(&self.compass_weights);
        self.archetype = result.name.clone();
        self.archetype_detail = Some(result);
        self.generate_sigil();
    }

    /// Produce soul data for the Genesis path contract without calling the orchestrator.
    pub fn soul_output(
        &self,
        name: &str,
        purpose: Option<&str>,
        personality: Option<&str>,
    ) -> Result<SoulOutput> {
        if self.state != AppState::Crystallize {
            anyhow::bail!(
                "soul_output requires state Crystallize (run scenarios and crystallize first)"
            );
        }

        let (default_purpose, default_personality) = self
            .archetype_detail
            .as_ref()
            .map(|a| (a.purpose.as_str(), a.personality.as_str()))
            .unwrap_or((
                "assist, retrieve, connect, and surface information",
                "balanced and adaptive; I read the situation and match what it needs",
            ));
        let purpose = purpose.unwrap_or(default_purpose);
        let personality = personality.unwrap_or(default_personality);

        let base_soul = orion_core::templates::fill_soul_template(name, purpose, personality);
        let mut soul_content = base_soul;
        soul_content.push_str("\n\n## Soul Forge Calibration\n\n");
        soul_content.push_str(&format!("**Archetype**: {}\n\n", self.archetype));
        soul_content.push_str("**Compass Ethic Weights**\n");
        soul_content.push_str(&format!(
            "- **Duty (North)**: {:.2}\n",
            self.compass_weights.duty
        ));
        soul_content.push_str(&format!(
            "- **Virtue (East)**: {:.2}\n",
            self.compass_weights.virtue
        ));
        soul_content.push_str(&format!(
            "- **Outcome (South)**: {:.2}\n",
            self.compass_weights.outcome
        ));
        soul_content.push_str(&format!(
            "- **Welfare (West)**: {:.2}\n",
            self.compass_weights.welfare
        ));
        soul_content.push_str(&format!(
            "\n*Prioritize {} in decisions.*\n",
            self.compass_weights.dominant()
        ));

        let growth_content = orion_core::templates::GROWTH_MD.to_string();

        let soul_json = serde_json::json!({
            "uuid": self.soul_hash,
            "archetype": self.archetype,
            "weights": {
                "duty": self.compass_weights.duty,
                "virtue": self.compass_weights.virtue,
                "outcome": self.compass_weights.outcome,
                "welfare": self.compass_weights.welfare,
            },
            "entropy": self.entropy_source,
            "session_seed": self.session_seed,
            "scenario_count": self.selected_scenarios.len(),
            "created_at": chrono::Utc::now().to_rfc3339()
        });

        Ok(SoulOutput {
            soul_content,
            growth_content,
            archetype: self.archetype.clone(),
            weights: self.weights.clone(),
            soul_hash: self.soul_hash.clone(),
            sigil_art: self.sigil_art.clone(),
            soul_json,
        })
    }

    /// Advance boot progress by one tick amount (for tests and TUI).
    pub fn tick_boot(&mut self, delta: u16) {
        if self.state == AppState::Boot {
            self.boot_progress = (self.boot_progress + delta).min(100);
        }
    }

    pub fn select_prev(&mut self) {
        if let AppState::Scenario(_) = self.state {
            let max = self
                .current_scenario()
                .map(|s| s.choices.len().saturating_sub(1))
                .unwrap_or(1);
            let i = self.list_state_selected.unwrap_or(0);
            self.list_state_selected = Some(if i == 0 { max } else { i - 1 });
        }
    }

    pub fn select_next(&mut self) {
        if let AppState::Scenario(_) = self.state {
            let max = self
                .current_scenario()
                .map(|s| s.choices.len().saturating_sub(1))
                .unwrap_or(1);
            let i = self.list_state_selected.unwrap_or(0);
            self.list_state_selected = Some(if i >= max { 0 } else { i + 1 });
        }
    }

    /// Standalone TUI path: run full birth flow and save soul using default name "Orion".
    pub fn save_soul(&self) -> Result<()> {
        let output = self.soul_output("Orion", None, None)?;

        let config = orion_core::AppConfig::default_paths();
        fs::create_dir_all(&config.data_dir)?;

        let orch_result = orion_birth::BirthOrchestrator::new(config.clone());

        match orch_result {
            Ok(mut orch) => {
                let docs_dir = orch.config().docs_dir.clone();
                if orch.current_stage() == orion_birth::BirthStage::Darkness {
                    orch.generate_identity(&docs_dir)?;
                    orch.advance_past_darkness()?;
                }
                if orch.current_stage() == orion_birth::BirthStage::Ignition {
                    orch.advance_to_connectivity()?;
                }
                if orch.current_stage() == orion_birth::BirthStage::Connectivity {
                    let config = orch.config();
                    let keyring = orion_core::ProviderKeyring::load(config.data_dir.clone())
                        .unwrap_or_else(|_| {
                            orion_core::ProviderKeyring::new(config.data_dir.clone())
                        });
                    let stored: Vec<String> = keyring
                        .list_providers()
                        .into_iter()
                        .map(String::from)
                        .collect();
                    let messages = orion_birth::build_birth_messages(&orch, &stored, Some("Hi"));
                    if !messages.is_empty() {
                        let rt = tokio::runtime::Runtime::new().unwrap();
                        let router = rt.block_on(orion_birth::build_birth_router(config));
                        if let Ok(response) =
                            rt.block_on(orion_birth::birth_chat_turn(&router, messages))
                        {
                            orch.add_message("user", "Hi");
                            orch.add_message("assistant", &response.assistant_content);
                        }
                    }
                    orch.advance_to_genesis()?;
                }

                orch.crystallize_soul(&output.soul_content, &output.growth_content)?;
                fs::write(
                    docs_dir.join("soul.json"),
                    serde_json::to_string_pretty(&output.soul_json)?,
                )?;
                orch.complete_emergence()?;
            }
            Err(_) => {
                let docs_dir = config.docs_dir.clone();
                fs::create_dir_all(&docs_dir)?;
                fs::write(docs_dir.join("soul.md"), &output.soul_content)?;
                fs::write(
                    docs_dir.join("soul.json"),
                    serde_json::to_string_pretty(&output.soul_json)?,
                )?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_new_state_is_boot() {
        let app = App::new();
        assert_eq!(app.state, AppState::Boot);
        assert_eq!(app.boot_progress, 0);
        assert_eq!(app.list_state_selected, Some(0));
    }

    #[test]
    fn test_app_default_has_5_scenarios() {
        let app = App::new();
        assert_eq!(app.selected_scenarios.len(), 5);
    }

    #[test]
    fn test_app_next_stage_boot_to_intro() {
        let mut app = App::new();
        app.state = AppState::Boot;
        app.next_stage();
        assert_eq!(app.state, AppState::Intro);
    }

    #[test]
    fn test_app_next_stage_full_flow_to_done() {
        let mut app = App::new();
        app.next_stage(); // Boot -> Intro
        assert_eq!(app.state, AppState::Intro);
        app.next_stage(); // Intro -> Scenario(0)
        assert_eq!(app.state, AppState::Scenario(0));

        // Walk through all scenarios
        for i in 0..app.scenario_count() {
            assert_eq!(app.state, AppState::Scenario(i));
            app.list_state_selected = Some(0);
            app.handle_selection();
        }

        assert_eq!(app.state, AppState::Crystallize);
        app.next_stage();
        assert_eq!(app.state, AppState::Done);
    }

    #[test]
    fn test_app_crystallize_deterministic_soul_hash() {
        let mut app1 = App::with_config(5, 42);
        app1.state = AppState::Intro;
        app1.next_stage(); // -> Scenario(0)
        for _ in 0..5 {
            app1.list_state_selected = Some(0);
            app1.handle_selection();
        }
        let hash1 = app1.soul_hash.clone();

        let mut app2 = App::with_config(5, 42);
        app2.state = AppState::Intro;
        app2.next_stage();
        for _ in 0..5 {
            app2.list_state_selected = Some(0);
            app2.handle_selection();
        }
        let hash2 = app2.soul_hash.clone();

        assert!(!hash1.is_empty());
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_app_tick_boot_only_in_boot() {
        let mut app = App::new();
        app.tick_boot(20);
        assert_eq!(app.boot_progress, 20);
        app.tick_boot(100);
        assert_eq!(app.boot_progress, 100);
        app.state = AppState::Intro;
        app.tick_boot(10);
        assert_eq!(app.boot_progress, 100);
    }

    #[test]
    fn test_soul_output_returns_valid_output() {
        let mut app = App::with_config(5, 123);
        app.next_stage(); // Boot -> Intro
        app.next_stage(); // Intro -> Scenario(0)
        for _ in 0..5 {
            app.list_state_selected = Some(0);
            app.handle_selection();
        }
        assert_eq!(app.state, AppState::Crystallize);

        let output = app.soul_output("TestAgent", None, None).unwrap();
        assert!(output.soul_content.contains("TestAgent"));
        assert!(output.soul_content.contains("Soul Forge Calibration"));
        assert!(output.soul_content.contains("Compass Ethic Weights"));
        assert!(!output.growth_content.is_empty());
        assert!(!output.archetype.is_empty());
        assert!(!output.soul_hash.is_empty());
    }

    #[test]
    fn test_compass_weights_used() {
        let mut app = App::with_config(5, 42);
        app.next_stage(); // Boot -> Intro
        app.next_stage(); // Intro -> Scenario(0)
        for _ in 0..5 {
            app.list_state_selected = Some(0);
            app.handle_selection();
        }
        // Weights should have been adjusted and finalized
        assert!(app.compass_weights.is_valid());
    }

    #[test]
    fn test_scenario_7_count() {
        let app = App::with_config(7, 42);
        assert_eq!(app.selected_scenarios.len(), 7);
    }
}
