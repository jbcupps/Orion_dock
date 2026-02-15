//! Integration test: birth orchestrator, build_birth_router, and birth_chat_turn wiring.

use orion_birth::{
    birth_chat_turn, build_birth_messages, build_birth_router, BirthOrchestrator, BirthStage,
};
use orion_core::AppConfig;
use std::path::Path;

fn test_config(base: &Path) -> AppConfig {
    let data_dir = base.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    AppConfig {
        schema_version: orion_core::CONFIG_SCHEMA_VERSION,
        data_dir: data_dir.clone(),
        models_dir: data_dir.join("models"),
        docs_dir: data_dir.join("docs"),
        db_path: data_dir.join("test.db"),
        openai_api_key: None,
        email: None,
        email_accounts: Default::default(),
        birth_complete: false,
        birth_stage: None,
        external_pubkey_path: None,
        local_llm_base_url: None,
        routing_mode: Default::default(),
        trinity: None,
        agent_name: None,
        birth_timestamp: None,
        mcp_servers: Default::default(),
        mcp_trust_policy: Default::default(),
        approved_skill_ids: Default::default(),
        trusted_skill_signers: Default::default(),
        sao_endpoint: None,
        memory_backend: Default::default(),
        database_url: None,
        birth_model: None,
        id_model_default: None,
        tier_models: std::collections::HashMap::new(),
        active_provider_preference: None,
        provider_catalog: std::collections::HashMap::new(),
    }
}

#[tokio::test]
async fn test_birth_orchestrator_to_router_wiring() {
    let tmp = std::env::temp_dir().join("orion_birth_integration_router");
    let _ = std::fs::remove_dir_all(&tmp);
    let config = test_config(&tmp);
    let docs_dir = config.docs_dir.clone();

    let mut orch = BirthOrchestrator::new(config.clone()).unwrap();
    assert_eq!(orch.current_stage(), BirthStage::Darkness);
    orch.generate_identity(&docs_dir).unwrap();
    orch.advance_past_darkness().unwrap();
    orch.advance_to_connectivity().unwrap();
    assert_eq!(orch.current_stage(), BirthStage::Connectivity);

    let router = build_birth_router(&config).await;
    let stored: Vec<String> = vec![];
    let messages = build_birth_messages(&orch, &stored, Some("Hi"));
    assert!(
        !messages.is_empty(),
        "Connectivity stage should yield messages"
    );

    let result = birth_chat_turn(&router, messages).await;
    assert!(
        result.is_err(),
        "stub Id returns Err for chat; wiring is correct"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
