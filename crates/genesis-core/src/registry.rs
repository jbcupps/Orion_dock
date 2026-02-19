//! GenesisRegistry: compile-time factory registration for genesis paths.

use crate::error::GenesisError;
use crate::strategy::{GenesisPathInfo, GenesisStrategy};
use std::collections::HashMap;
use std::sync::Arc;

/// Factory function that creates a new strategy instance.
pub type StrategyFactory = Arc<dyn Fn() -> Box<dyn GenesisStrategy> + Send + Sync>;

/// Registry of available genesis paths.
pub struct GenesisRegistry {
    factories: HashMap<String, StrategyFactory>,
    infos: Vec<GenesisPathInfo>,
}

impl GenesisRegistry {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
            infos: Vec::new(),
        }
    }

    /// Register a genesis path with a factory function.
    pub fn register<F>(&mut self, id: &str, factory: F)
    where
        F: Fn() -> Box<dyn GenesisStrategy> + Send + Sync + 'static,
    {
        let factory = Arc::new(factory);
        // Create a temporary instance to get info
        let temp = factory();
        let info = temp.info();
        self.factories.insert(id.to_string(), factory);
        self.infos.push(info);
    }

    /// Create a new strategy instance by path id.
    pub fn create(&self, id: &str) -> Result<Box<dyn GenesisStrategy>, GenesisError> {
        let factory = self
            .factories
            .get(id)
            .ok_or_else(|| GenesisError::PathNotFound(id.to_string()))?;
        Ok(factory())
    }

    /// List all registered paths.
    pub fn list_paths(&self) -> &[GenesisPathInfo] {
        &self.infos
    }

    /// Check if a path id is registered.
    pub fn has_path(&self, id: &str) -> bool {
        self.factories.contains_key(id)
    }
}

impl Default for GenesisRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::*;
    use async_trait::async_trait;

    struct DummyStrategy;

    #[async_trait]
    impl GenesisStrategy for DummyStrategy {
        fn info(&self) -> GenesisPathInfo {
            GenesisPathInfo {
                id: "dummy".to_string(),
                label: "Dummy".to_string(),
                description: "Test path".to_string(),
                estimated_time: "~0s".to_string(),
                requires_chat: false,
                variants: vec![],
            }
        }

        async fn initialize(&mut self, _ctx: GenesisContext) -> Result<(), GenesisError> {
            Ok(())
        }

        async fn begin(&mut self) -> Result<StepRequest, GenesisError> {
            Ok(StepRequest::Complete {
                manifest: Box::new(crate::manifest::SoulManifest {
                    agent_name: "Test".into(),
                    mentor_name: "Mentor".into(),
                    archetype: "Balanced".into(),
                    soul_content: "".into(),
                    growth_content: "".into(),
                    ethics_weights: crate::ethics::CompassEthicWeights::default(),
                    birth_method: "dummy".into(),
                    genesis_hash: "abc".into(),
                    archetype_detail: None,
                    sigil_art: None,
                    raw_signals: None,
                    soul_json: None,
                }),
            })
        }

        async fn advance(&mut self, _input: StepResponse) -> Result<StepRequest, GenesisError> {
            Err(GenesisError::AlreadyComplete)
        }

        fn snapshot(&self) -> Result<serde_json::Value, GenesisError> {
            Ok(serde_json::json!({}))
        }

        fn restore(&mut self, _snapshot: serde_json::Value) -> Result<(), GenesisError> {
            Ok(())
        }

        fn cancel(&mut self) {}
    }

    #[test]
    fn test_registry_register_and_create() {
        let mut reg = GenesisRegistry::new();
        reg.register("dummy", || Box::new(DummyStrategy));
        assert!(reg.has_path("dummy"));
        assert!(!reg.has_path("nonexistent"));
        assert_eq!(reg.list_paths().len(), 1);
        assert_eq!(reg.list_paths()[0].id, "dummy");

        let strategy = reg.create("dummy");
        assert!(strategy.is_ok());

        let err = reg.create("nonexistent");
        assert!(err.is_err());
    }
}
