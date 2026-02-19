//! Quick Start genesis path: auto-generates standard identity with no ceremony.
//!
//! Immediately produces a SoulManifest using default templates.
//! No chat interaction required.

use async_trait::async_trait;
use genesis_core::{
    CompassEthicWeights, GenesisContext, GenesisError, GenesisPathInfo, GenesisStrategy,
    SoulManifest, StepRequest, StepResponse,
};

/// Quick Start strategy: instant soul generation with defaults.
pub struct QuickStartStrategy {
    ctx: Option<GenesisContext>,
    complete: bool,
}

impl QuickStartStrategy {
    pub fn new() -> Self {
        Self {
            ctx: None,
            complete: false,
        }
    }
}

impl Default for QuickStartStrategy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl GenesisStrategy for QuickStartStrategy {
    fn info(&self) -> GenesisPathInfo {
        GenesisPathInfo {
            id: "quick_start".to_string(),
            label: "Quick Start".to_string(),
            description: "Skip the ceremony. Auto-generate a standard identity and constitutional \
                          documents from the agent name. You can customize later."
                .to_string(),
            estimated_time: "~5 seconds".to_string(),
            requires_chat: false,
            variants: vec![],
        }
    }

    async fn initialize(&mut self, ctx: GenesisContext) -> Result<(), GenesisError> {
        self.ctx = Some(ctx);
        Ok(())
    }

    async fn begin(&mut self) -> Result<StepRequest, GenesisError> {
        let ctx = self.ctx.as_ref().ok_or(GenesisError::NotInitialized)?;
        if self.complete {
            return Err(GenesisError::AlreadyComplete);
        }

        let agent_name = &ctx.agent_name;
        let mentor_name = &ctx.mentor_name;

        let soul_content = orion_core::templates::fill_soul_template_default(agent_name);
        let growth_content = orion_core::templates::GROWTH_MD.to_string();

        let weights = CompassEthicWeights::default();
        let archetype = "The Balanced Synthesist".to_string();

        let genesis_hash = SoulManifest::compute_genesis_hash(
            agent_name,
            &archetype,
            &weights,
            "Quick Start",
            std::slice::from_ref(mentor_name),
        );

        let manifest = SoulManifest {
            agent_name: agent_name.clone(),
            mentor_name: mentor_name.clone(),
            archetype,
            soul_content,
            growth_content,
            ethics_weights: weights,
            birth_method: "Quick Start".to_string(),
            genesis_hash,
            archetype_detail: Some(
                "balanced and adaptive; I read the situation and match what it needs".to_string(),
            ),
            sigil_art: None,
            raw_signals: None,
            soul_json: None,
        };

        self.complete = true;
        Ok(StepRequest::Complete {
            manifest: Box::new(manifest),
        })
    }

    async fn advance(&mut self, _input: StepResponse) -> Result<StepRequest, GenesisError> {
        Err(GenesisError::AlreadyComplete)
    }

    fn snapshot(&self) -> Result<serde_json::Value, GenesisError> {
        Ok(serde_json::json!({
            "complete": self.complete,
        }))
    }

    fn restore(&mut self, snapshot: serde_json::Value) -> Result<(), GenesisError> {
        self.complete = snapshot
            .get("complete")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        Ok(())
    }

    fn cancel(&mut self) {
        self.complete = false;
        self.ctx = None;
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
            scenario_count: None,
        }
    }

    #[test]
    fn test_info() {
        let s = QuickStartStrategy::new();
        let info = s.info();
        assert_eq!(info.id, "quick_start");
        assert!(!info.requires_chat);
    }

    #[tokio::test]
    async fn test_begin_returns_complete() {
        let mut s = QuickStartStrategy::new();
        s.initialize(test_ctx()).await.unwrap();
        let result = s.begin().await.unwrap();
        match result {
            StepRequest::Complete { manifest } => {
                assert_eq!(manifest.agent_name, "TestAgent");
                assert_eq!(manifest.mentor_name, "Jason");
                assert_eq!(manifest.archetype, "The Balanced Synthesist");
                assert_eq!(manifest.birth_method, "Quick Start");
                assert!(manifest.ethics_weights.is_valid());
                assert!(!manifest.genesis_hash.is_empty());
                assert!(manifest.soul_content.contains("TestAgent"));
            }
            _ => panic!("Expected Complete"),
        }
    }

    #[tokio::test]
    async fn test_advance_after_complete_errors() {
        let mut s = QuickStartStrategy::new();
        s.initialize(test_ctx()).await.unwrap();
        s.begin().await.unwrap();
        let result = s
            .advance(StepResponse::Message {
                text: "hi".to_string(),
            })
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_snapshot_restore() {
        let mut s = QuickStartStrategy::new();
        s.initialize(test_ctx()).await.unwrap();
        s.begin().await.unwrap();
        let snap = s.snapshot().unwrap();
        assert_eq!(snap["complete"], true);

        let mut s2 = QuickStartStrategy::new();
        s2.restore(snap).unwrap();
        assert!(s2.complete);
    }
}
