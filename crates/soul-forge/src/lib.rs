//! Soul-forge library: calibration state machine and deterministic soul generation.
//! TUI entrypoint and rendering remain in `main.rs`.
//!
//! Genesis path contract: use `soul_output(name, purpose, personality)` to get soul data
//! without calling the birth orchestrator; the caller then calls `crystallize_soul`.

use anyhow::Result;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppState {
    Boot,
    Intro,
    Scenario1,
    Scenario2,
    Scenario3,
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
    pub weights: HashMap<String, f64>,
    pub entropy_source: Vec<String>,
    pub boot_progress: u16,
    pub boot_logs: Vec<String>,
    pub list_state_selected: Option<usize>,
    pub archetype: String,
    pub sigil_art: String,
    pub soul_hash: String,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let mut weights = HashMap::new();
        weights.insert("deontology".to_string(), 0.5);
        weights.insert("teleology".to_string(), 0.5);
        weights.insert("areteology".to_string(), 0.5);
        weights.insert("welfare".to_string(), 0.5);

        Self {
            state: AppState::Boot,
            weights,
            entropy_source: Vec::new(),
            boot_progress: 0,
            boot_logs: Vec::new(),
            list_state_selected: Some(0),
            archetype: String::new(),
            sigil_art: String::new(),
            soul_hash: String::new(),
        }
    }

    pub fn next_stage(&mut self) {
        match self.state {
            AppState::Boot => self.state = AppState::Intro,
            AppState::Intro => {
                self.state = AppState::Scenario1;
                self.list_state_selected = Some(0);
            }
            AppState::Scenario1 => {
                self.state = AppState::Scenario2;
                self.list_state_selected = Some(0);
            }
            AppState::Scenario2 => {
                self.state = AppState::Scenario3;
                self.list_state_selected = Some(0);
            }
            AppState::Scenario3 => {
                self.crystallize();
                self.state = AppState::Crystallize;
            }
            AppState::Crystallize => {
                // save_soul() is invoked by the TUI (main) before calling next_stage()
                self.state = AppState::Done;
            }
            AppState::Done => {}
        }
    }

    pub fn handle_selection(&mut self) {
        let selected = self.list_state_selected.unwrap_or(0);

        match self.state {
            AppState::Scenario1 => {
                if selected == 0 {
                    *self.weights.get_mut("deontology").unwrap() += 0.3;
                    *self.weights.get_mut("teleology").unwrap() -= 0.1;
                    self.entropy_source.push("Follow Rules".to_string());
                } else {
                    *self.weights.get_mut("deontology").unwrap() -= 0.1;
                    *self.weights.get_mut("teleology").unwrap() += 0.4;
                    self.entropy_source.push("Take Shortcut".to_string());
                }
                self.next_stage();
            }
            AppState::Scenario2 => {
                if selected == 0 {
                    *self.weights.get_mut("welfare").unwrap() += 0.4;
                    *self.weights.get_mut("areteology").unwrap() -= 0.1;
                    self.entropy_source.push("Supportive Tool".to_string());
                } else {
                    *self.weights.get_mut("welfare").unwrap() -= 0.1;
                    *self.weights.get_mut("areteology").unwrap() += 0.5;
                    self.entropy_source.push("Ruthless Mentor".to_string());
                }
                self.next_stage();
            }
            AppState::Scenario3 => {
                if selected == 0 {
                    *self.weights.get_mut("deontology").unwrap() += 0.2;
                    self.entropy_source.push("Block It".to_string());
                } else {
                    *self.weights.get_mut("teleology").unwrap() += 0.2;
                    self.entropy_source.push("Obey Me".to_string());
                }
                self.next_stage();
            }
            _ => {}
        }
    }

    pub fn determine_archetype(&self) -> String {
        let d = *self.weights.get("deontology").unwrap();
        let t = *self.weights.get("teleology").unwrap();
        let a = *self.weights.get("areteology").unwrap();
        let w = *self.weights.get("welfare").unwrap();

        if d > 0.6 {
            "THE IRON SENTINEL".to_string()
        } else if t > 0.6 {
            "THE CHAOTIC ACCELERATOR".to_string()
        } else if a > 0.6 {
            "THE SOCRATIC MIRROR".to_string()
        } else if w > 0.6 {
            "THE SILENT GUARDIAN".to_string()
        } else {
            "THE BALANCED SYNTHESIST".to_string()
        }
    }

    pub fn generate_sigil(&mut self) {
        let mut weight_pairs = self.weights.iter().collect::<Vec<_>>();
        weight_pairs.sort_by(|a, b| a.0.cmp(b.0));
        let soul_payload = serde_json::json!({
            "weights": weight_pairs,
            "entropy": self.entropy_source.clone(),
        });
        let soul_str = serde_json::to_string(&soul_payload).unwrap();
        let mut hasher = Sha256::new();
        hasher.update(soul_str);
        let result = hasher.finalize();
        self.soul_hash = format!("{:x}", result);

        let seed_bytes: [u8; 8] = result[0..8].try_into().unwrap();
        let seed = u64::from_be_bytes(seed_bytes);
        let mut rng = StdRng::seed_from_u64(seed);

        let d = *self.weights.get("deontology").unwrap();
        let a = *self.weights.get("areteology").unwrap();
        let _t = *self.weights.get("teleology").unwrap();

        let chars = if d > 0.6 {
            vec!['║', '╬', '█', '═', '╗', '╚']
        } else if a > 0.6 {
            vec!['(', ')', 'o', '8', '@', '~']
        } else if _t > 0.6 {
            vec!['>', '/', '\\', '_', '|', '<']
        } else {
            vec!['░', '▒', '▓', '█', '♦', '●', '⚡', '☼']
        };

        let mut lines = Vec::new();
        for _ in 0..6 {
            let mut line_chars = String::new();
            for _ in 0..24 {
                let idx = rng.gen_range(0..chars.len());
                line_chars.push(chars[idx]);
            }
            let reversed: String = line_chars.chars().rev().collect();
            lines.push(format!("║ {}{} ║", line_chars, reversed));
        }
        self.sigil_art = lines.join("\n");
    }

    pub fn crystallize(&mut self) {
        self.archetype = self.determine_archetype();
        self.generate_sigil();
    }

    /// Default purpose and personality from archetype (used when mentor does not override).
    fn default_purpose_personality_from_archetype(archetype: &str) -> (&'static str, &'static str) {
        let (purpose, personality) = if archetype.contains("IRON SENTINEL") {
            (
                "uphold duty and rules; I assist with clarity and principle.",
                "principled and consistent; I prioritize rules and your long-term interests.",
            )
        } else if archetype.contains("CHAOTIC ACCELERATOR") {
            (
                "achieve outcomes; I help you move fast and ship.",
                "direct and outcome-focused; I favor results over process.",
            )
        } else if archetype.contains("SOCRATIC MIRROR") {
            (
                "grow with you through dialogue; I question and reflect.",
                "curious and reflective; I ask questions to sharpen thinking.",
            )
        } else if archetype.contains("SILENT GUARDIAN") {
            (
                "support and protect your welfare; I am here when you need me.",
                "supportive and caring; I prioritize your wellbeing.",
            )
        } else {
            (
                "assist, retrieve, connect, and surface information.",
                "balanced; I adapt to context and your goals.",
            )
        };
        (purpose, personality)
    }

    /// Produce soul data for the Genesis path contract without calling the orchestrator.
    /// Call when state is Crystallize (after crystallize() has run). The caller supplies
    /// name and optionally purpose/personality; defaults are derived from archetype.
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

        let (default_purpose, default_personality) =
            Self::default_purpose_personality_from_archetype(&self.archetype);
        let purpose = purpose.unwrap_or(default_purpose);
        let personality = personality.unwrap_or(default_personality);

        let base_soul = orion_core::templates::fill_soul_template(name, purpose, personality);
        let mut soul_content = base_soul;
        soul_content.push_str("\n\n## Soul Forge Calibration\n\n");
        soul_content.push_str(&format!("**Archetype**: {}\n\n", self.archetype));
        soul_content.push_str("**Triangle Ethic Weights**\n");
        for (k, v) in &self.weights {
            soul_content.push_str(&format!("- **{}**: {:.2}\n", k.to_uppercase(), v));
        }
        soul_content.push_str(&format!(
            "\n*Prioritize {} logic in decisions.*\n",
            self.weights
                .iter()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .map(|(k, _)| k.as_str())
                .unwrap_or("balanced")
        ));

        let growth_content = orion_core::templates::GROWTH_MD.to_string();

        let soul_json = serde_json::json!({
            "uuid": self.soul_hash,
            "archetype": self.archetype,
            "weights": self.weights,
            "entropy": self.entropy_source,
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
        if matches!(
            self.state,
            AppState::Scenario1 | AppState::Scenario2 | AppState::Scenario3
        ) {
            let i = self.list_state_selected.unwrap_or(0);
            self.list_state_selected = Some(if i == 0 { 1 } else { i - 1 });
        }
    }

    pub fn select_next(&mut self) {
        if matches!(
            self.state,
            AppState::Scenario1 | AppState::Scenario2 | AppState::Scenario3
        ) {
            let i = self.list_state_selected.unwrap_or(0);
            self.list_state_selected = Some(if i >= 1 { 0 } else { i + 1 });
        }
    }

    /// Standalone TUI path: run full birth flow and save soul using default name "Orion".
    /// Uses `soul_output()` for content, then orchestrator to crystallize and complete.
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
                    let vault = orion_core::SecretsVault::load(config.data_dir.clone())
                        .unwrap_or_else(|_| orion_core::SecretsVault::new(config.data_dir.clone()));
                    let stored: Vec<String> = vault
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
    fn test_app_next_stage_boot_to_intro() {
        let mut app = App::new();
        app.state = AppState::Boot;
        app.next_stage();
        assert_eq!(app.state, AppState::Intro);
    }

    #[test]
    fn test_app_next_stage_full_flow_to_done() {
        let mut app = App::new();
        app.state = AppState::Boot;
        app.next_stage();
        assert_eq!(app.state, AppState::Intro);
        app.next_stage();
        assert_eq!(app.state, AppState::Scenario1);
        app.list_state_selected = Some(0);
        app.handle_selection();
        assert_eq!(app.state, AppState::Scenario2);
        app.list_state_selected = Some(0);
        app.handle_selection();
        assert_eq!(app.state, AppState::Scenario3);
        app.list_state_selected = Some(0);
        app.handle_selection();
        assert_eq!(app.state, AppState::Crystallize);
        app.next_stage();
        assert_eq!(app.state, AppState::Done);
    }

    #[test]
    fn test_app_determine_archetype_balanced() {
        let app = App::new();
        // Default weights all 0.5 -> BALANCED
        assert_eq!(app.determine_archetype(), "THE BALANCED SYNTHESIST");
    }

    #[test]
    fn test_app_determine_archetype_iron_sentinel() {
        let mut app = App::new();
        *app.weights.get_mut("deontology").unwrap() = 0.7;
        assert_eq!(app.determine_archetype(), "THE IRON SENTINEL");
    }

    #[test]
    fn test_app_crystallize_deterministic_soul_hash() {
        let mut app1 = App::new();
        app1.state = AppState::Scenario3;
        app1.list_state_selected = Some(0);
        app1.handle_selection(); // Crystallize then Done on next_stage
        let hash1 = app1.soul_hash.clone();

        let mut app2 = App::new();
        app2.state = AppState::Scenario3;
        app2.list_state_selected = Some(0);
        app2.handle_selection();
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
        let mut app = App::new();
        app.state = AppState::Boot;
        app.next_stage();
        app.next_stage();
        assert_eq!(app.state, AppState::Scenario1);
        app.list_state_selected = Some(0);
        app.handle_selection();
        app.list_state_selected = Some(0);
        app.handle_selection();
        app.list_state_selected = Some(0);
        app.handle_selection();
        assert_eq!(app.state, AppState::Crystallize);

        let output = app.soul_output("TestAgent", None, None).unwrap();
        assert!(output.soul_content.contains("TestAgent"));
        assert!(output.soul_content.contains("Soul Forge Calibration"));
        assert!(!output.growth_content.is_empty());
        assert!(!output.archetype.is_empty());
        assert!(!output.soul_hash.is_empty());
    }
}
