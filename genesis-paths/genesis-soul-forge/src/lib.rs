//! Soul Forge genesis path: scenario-based ethical calibration with archetypes and sigils.
//!
//! Wraps `soul_forge::App` into a `GenesisStrategy`. Presents TOML-defined ethical
//! scenarios as discrete choices, then produces a SoulManifest with archetype,
//! compass weights, sigil art, and soul hash.

use async_trait::async_trait;
use genesis_core::{
    ChoiceOption, CompassEthicWeights, GenesisContext, GenesisError, GenesisPathInfo,
    GenesisStrategy, SoulManifest, StepRequest, StepResponse,
};
use soul_forge::{App, AppState};

/// Soul Forge strategy: scenario-based calibration ritual.
pub struct SoulForgeStrategy {
    ctx: Option<GenesisContext>,
    app: Option<App>,
    complete: bool,
    intro_shown: bool,
}

impl SoulForgeStrategy {
    pub fn new() -> Self {
        Self {
            ctx: None,
            app: None,
            complete: false,
            intro_shown: false,
        }
    }
}

impl Default for SoulForgeStrategy {
    fn default() -> Self {
        Self::new()
    }
}

fn scenario_to_step_request(app: &App, scenario_index: usize) -> StepRequest {
    let scenario = &app.selected_scenarios[scenario_index];
    let choices: Vec<ChoiceOption> = scenario
        .choices
        .iter()
        .map(|c| ChoiceOption {
            label: c.label.clone(),
            description: c.description.clone(),
        })
        .collect();

    let total = app.scenario_count();
    StepRequest::NeedChoice {
        prompt: format!(
            "**Scenario {} of {}**: {}\n\n{}",
            scenario_index + 1,
            total,
            scenario.title,
            scenario.prompt,
        ),
        choices,
        metadata: Some(serde_json::json!({
            "scenario_index": scenario_index,
            "scenario_total": total,
            "scenario_id": scenario.id,
            "category": scenario.category,
            "difficulty": scenario.difficulty,
        })),
    }
}

#[async_trait]
impl GenesisStrategy for SoulForgeStrategy {
    fn info(&self) -> GenesisPathInfo {
        GenesisPathInfo {
            id: "soul_forge".to_string(),
            label: "Soul Forge".to_string(),
            description: "I will present ethical scenarios. Do not overthink them. React. \
                          Your instinctive choices will calibrate my ethical weights and \
                          determine my archetype. At the end, you will receive a Soul Sigil \
                          — a unique cryptographic artifact derived from your specific answers."
                .to_string(),
            estimated_time: "~2 minutes".to_string(),
            requires_chat: false,
            variants: vec![],
        }
    }

    async fn initialize(&mut self, ctx: GenesisContext) -> Result<(), GenesisError> {
        let scenario_count = ctx.scenario_count.unwrap_or(5);
        let timestamp = chrono::Utc::now().to_rfc3339();
        let seed = soul_forge::session::session_seed(&ctx.agent_name, &timestamp);
        let app = App::with_config(scenario_count, seed);
        self.app = Some(app);
        self.ctx = Some(ctx);
        Ok(())
    }

    async fn begin(&mut self) -> Result<StepRequest, GenesisError> {
        self.ctx.as_ref().ok_or(GenesisError::NotInitialized)?;
        let app = self.app.as_mut().ok_or(GenesisError::NotInitialized)?;
        if self.complete {
            return Err(GenesisError::AlreadyComplete);
        }

        // Advance from Boot to Intro
        if app.state == AppState::Boot {
            app.state = AppState::Intro;
        }

        self.intro_shown = true;

        // Present intro as a single-choice "Continue" prompt
        Ok(StepRequest::NeedChoice {
            prompt: "Welcome to the **Soul Forge**.\n\n\
                     I will present you with a series of scenarios. Each describes a situation \
                     I might face as your agent. Choose the response that feels most natural \
                     to you — there are no wrong answers.\n\n\
                     Your choices will calibrate my ethical compass and determine my archetype."
                .to_string(),
            choices: vec![ChoiceOption {
                label: "Begin".to_string(),
                description: "Start the calibration scenarios".to_string(),
            }],
            metadata: Some(serde_json::json!({
                "stage": "intro",
                "scenario_total": app.scenario_count(),
            })),
        })
    }

    async fn advance(&mut self, input: StepResponse) -> Result<StepRequest, GenesisError> {
        self.ctx.as_ref().ok_or(GenesisError::NotInitialized)?;
        let app = self.app.as_mut().ok_or(GenesisError::NotInitialized)?;
        if self.complete {
            return Err(GenesisError::AlreadyComplete);
        }

        let choice_index = match input {
            StepResponse::Choice { index } => index,
            _ => {
                return Err(GenesisError::InvalidInput(
                    "Expected a choice selection".to_string(),
                ))
            }
        };

        match app.state {
            AppState::Intro => {
                // "Begin" was selected; advance to first scenario
                app.next_stage();
                if let AppState::Scenario(idx) = app.state {
                    return Ok(scenario_to_step_request(app, idx));
                }
            }
            AppState::Scenario(_) => {
                // Apply the chosen answer
                app.list_state_selected = Some(choice_index);
                app.handle_selection();

                match app.state {
                    AppState::Scenario(idx) => {
                        return Ok(scenario_to_step_request(app, idx));
                    }
                    AppState::Crystallize => {
                        // All scenarios done, produce manifest
                        return self.produce_manifest();
                    }
                    _ => {}
                }
            }
            AppState::Crystallize => {
                return self.produce_manifest();
            }
            _ => {}
        }

        Err(GenesisError::InvalidState(format!(
            "Unexpected state: {:?}",
            app.state
        )))
    }

    fn snapshot(&self) -> Result<serde_json::Value, GenesisError> {
        let app_state = self.app.as_ref().map(|app| {
            serde_json::json!({
                "state": format!("{:?}", app.state),
                "compass_weights": {
                    "duty": app.compass_weights.duty,
                    "virtue": app.compass_weights.virtue,
                    "outcome": app.compass_weights.outcome,
                    "welfare": app.compass_weights.welfare,
                },
                "entropy_source": app.entropy_source,
                "current_scenario_index": app.current_scenario_index,
                "session_seed": app.session_seed,
                "scenario_count": app.scenario_count(),
            })
        });
        Ok(serde_json::json!({
            "complete": self.complete,
            "intro_shown": self.intro_shown,
            "app": app_state,
        }))
    }

    fn restore(&mut self, snapshot: serde_json::Value) -> Result<(), GenesisError> {
        self.complete = snapshot
            .get("complete")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        self.intro_shown = snapshot
            .get("intro_shown")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // Note: full App state restoration would need serializable App;
        // for now we capture enough for diagnostics.
        Ok(())
    }

    fn cancel(&mut self) {
        self.complete = false;
        self.app = None;
        self.ctx = None;
    }
}

impl SoulForgeStrategy {
    fn produce_manifest(&mut self) -> Result<StepRequest, GenesisError> {
        // Extract context values first
        let (agent_name, mentor_name) = {
            let ctx = self.ctx.as_ref().ok_or(GenesisError::NotInitialized)?;
            (ctx.agent_name.clone(), ctx.mentor_name.clone())
        };

        let app = self.app.as_ref().ok_or(GenesisError::NotInitialized)?;

        let output = app
            .soul_output(&agent_name, None, None)
            .map_err(|e| GenesisError::Other(e.to_string()))?;

        let weights = CompassEthicWeights::new(
            app.compass_weights.duty,
            app.compass_weights.virtue,
            app.compass_weights.outcome,
            app.compass_weights.welfare,
        );

        let archetype_detail = app.archetype_detail.as_ref().map(|a| {
            format!(
                "Purpose: {}\nPersonality: {}\nPrimary: {:?}, Secondary: {:?}",
                a.purpose, a.personality, a.primary, a.secondary
            )
        });

        let genesis_hash = SoulManifest::compute_genesis_hash(
            &agent_name,
            &output.archetype,
            &weights,
            "Soul Forge v2",
            &output
                .weights
                .iter()
                .map(|(k, v)| format!("{}:{:.4}", k, v))
                .collect::<Vec<_>>(),
        );

        let manifest = SoulManifest {
            agent_name,
            mentor_name,
            archetype: output.archetype,
            soul_content: output.soul_content,
            growth_content: output.growth_content,
            ethics_weights: weights,
            birth_method: "Soul Forge v2".to_string(),
            genesis_hash,
            archetype_detail,
            sigil_art: Some(output.sigil_art),
            raw_signals: None,
            soul_json: Some(output.soul_json),
        };

        self.complete = true;
        Ok(StepRequest::Complete {
            manifest: Box::new(manifest),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_ctx() -> GenesisContext {
        GenesisContext {
            agent_name: "TestAgent".to_string(),
            mentor_name: "Jason".to_string(),
            docs_dir: PathBuf::from("/tmp/test-docs"),
            data_dir: PathBuf::from("/tmp/test-data"),
            chat_fn: None,
            variant: None,
            scenario_count: Some(5),
        }
    }

    #[test]
    fn test_info() {
        let s = SoulForgeStrategy::new();
        let info = s.info();
        assert_eq!(info.id, "soul_forge");
        assert!(!info.requires_chat);
    }

    #[tokio::test]
    async fn test_begin_shows_intro() {
        let mut s = SoulForgeStrategy::new();
        s.initialize(test_ctx()).await.unwrap();
        let result = s.begin().await.unwrap();
        match result {
            StepRequest::NeedChoice {
                prompt,
                choices,
                metadata,
                ..
            } => {
                assert!(prompt.contains("Soul Forge"));
                assert_eq!(choices.len(), 1);
                assert_eq!(choices[0].label, "Begin");
                assert!(metadata.is_some());
            }
            _ => panic!("Expected NeedChoice for intro"),
        }
    }

    #[tokio::test]
    async fn test_full_forge_flow() {
        let mut s = SoulForgeStrategy::new();
        s.initialize(test_ctx()).await.unwrap();

        // Begin (intro)
        let result = s.begin().await.unwrap();
        assert!(matches!(result, StepRequest::NeedChoice { .. }));

        // Select "Begin"
        let result = s.advance(StepResponse::Choice { index: 0 }).await.unwrap();

        // Should be first scenario
        match &result {
            StepRequest::NeedChoice { metadata, .. } => {
                let idx = metadata
                    .as_ref()
                    .and_then(|m| m.get("scenario_index"))
                    .and_then(|v| v.as_u64());
                assert_eq!(idx, Some(0));
            }
            _ => panic!("Expected NeedChoice for scenario 0"),
        }

        // Walk through remaining scenarios (always pick choice 0)
        let mut last_result = result;
        for _ in 0..5 {
            match &last_result {
                StepRequest::NeedChoice { .. } => {
                    last_result = s.advance(StepResponse::Choice { index: 0 }).await.unwrap();
                }
                StepRequest::Complete { .. } => break,
                _ => panic!("Unexpected step type"),
            }
        }

        // Should end with Complete
        match last_result {
            StepRequest::Complete { manifest } => {
                assert_eq!(manifest.agent_name, "TestAgent");
                assert_eq!(manifest.mentor_name, "Jason");
                assert_eq!(manifest.birth_method, "Soul Forge v2");
                assert!(manifest.ethics_weights.is_valid());
                assert!(!manifest.archetype.is_empty());
                assert!(manifest.sigil_art.is_some());
                assert!(manifest.soul_json.is_some());
            }
            _ => panic!("Expected Complete after all scenarios"),
        }
    }

    #[tokio::test]
    async fn test_advance_after_complete_errors() {
        let mut s = SoulForgeStrategy::new();
        s.initialize(test_ctx()).await.unwrap();
        s.begin().await.unwrap();

        // Advance through intro + 5 scenarios
        s.advance(StepResponse::Choice { index: 0 }).await.unwrap();
        for _ in 0..5 {
            let _ = s.advance(StepResponse::Choice { index: 0 }).await;
        }

        // Now should error
        let result = s.advance(StepResponse::Choice { index: 0 }).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_snapshot() {
        let s = SoulForgeStrategy::new();
        let snap = s.snapshot().unwrap();
        assert_eq!(snap["complete"], false);
    }
}
