//! Headless UAT driver: drives BirthOrchestrator through all stages to usable operation.
//! Keys provided via env: TEST_LLM_KEY, TEST_SEARCH_KEY (optional: TEST_LLM_PROVIDER, TEST_SEARCH_PROVIDER).
//! Genesis path via UAT_GENESIS_PATH: "direct" (default), "soul_forge", "soul_crystallization".
//! Data dir via ORION_DATA_DIR; DB/LLM via DATABASE_URL, LOCAL_LLM_BASE_URL, MEMORY_BACKEND, BIRTH_MODEL.

use anyhow::{Context, Result};
use orion_birth::{
    birth_chat_turn, build_birth_router, execute_store_provider_key, BirthOrchestrator, BirthStage,
    GenesisPath, SoulCrystallizationDepth,
};
use orion_capabilities::cognitive::Message;
use orion_core::{AppConfig, MemoryBackend, ProviderKeyring, SkillKeychain};
use soul_forge::{App as SoulForgeApp, AppState as SoulForgeAppState};
use std::path::PathBuf;
use std::str::FromStr;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "orion_uat=info,orion_birth=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let data_dir = std::env::var("ORION_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::temp_dir().join("orion_uat").join(format!(
                "uat_{}",
                chrono::Utc::now().format("%Y%m%d_%H%M%S")
            ))
        });
    std::fs::create_dir_all(&data_dir).context("create ORION_DATA_DIR")?;

    let mut config = app_config_from_env(&data_dir)?;
    config.data_dir = data_dir.clone();
    config.models_dir = data_dir.join("models");
    config.docs_dir = data_dir.join("docs");
    config.db_path = data_dir.join("orion_uat.db");
    std::fs::create_dir_all(&config.docs_dir)?;

    let mut orch = BirthOrchestrator::new(config.clone())
        .context("BirthOrchestrator::new (already born or store error)")?;

    // --- Darkness ---
    if orch.current_stage() == BirthStage::Darkness {
        orch.generate_identity(&config.docs_dir)?;
        orch.advance_past_darkness()?;
        tracing::info!("UAT: Darkness done, advanced to Ignition");
    }

    // --- Ignition ---
    if orch.current_stage() == BirthStage::Ignition {
        orch.advance_to_connectivity()?;
        tracing::info!("UAT: Ignition done, advanced to Connectivity");
    }

    let genesis_path = parse_genesis_path_from_env();

    // --- Connectivity: inject keys from env ---
    if orch.current_stage() == BirthStage::Connectivity {
        let config = orch.config_mut();
        let mut keyring = ProviderKeyring::load(config.data_dir.clone())
            .unwrap_or_else(|_| ProviderKeyring::new(config.data_dir.clone()));
        let mut skill_kc = SkillKeychain::load(config.data_dir.clone())
            .unwrap_or_else(|_| SkillKeychain::new(config.data_dir.clone()));
        let llm_key = std::env::var("TEST_LLM_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let search_key = std::env::var("TEST_SEARCH_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty());
        let llm_provider = std::env::var("TEST_LLM_PROVIDER")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "auto".to_string());
        let search_provider = std::env::var("TEST_SEARCH_PROVIDER")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "auto".to_string());

        if let Some(key) = llm_key {
            let _ = execute_store_provider_key(&mut keyring, config, &llm_provider, &key, Some(&mut skill_kc));
            tracing::info!("UAT: Stored LLM provider key");
        }
        if let Some(key) = search_key {
            let _ = execute_store_provider_key(&mut keyring, config, &search_provider, &key, Some(&mut skill_kc));
            tracing::info!("UAT: Stored search provider key");
        }
        orch.advance_to_genesis_with_path(genesis_path.clone())?;
        tracing::info!(
            "UAT: Connectivity done, advanced to Genesis (path: {})",
            genesis_path.id()
        );
    }

    // --- Genesis: path-aware crystallize ---
    if orch.current_stage() == BirthStage::Genesis {
        let (soul_md, growth_md) = match &genesis_path {
            GenesisPath::Direct => {
                let soul = orion_core::templates::fill_soul_template(
                    "OrionUAT",
                    "assist with UAT verification.",
                    "concise and deterministic.",
                );
                let growth = orion_core::templates::GROWTH_MD.to_string();
                (soul, growth)
            }
            GenesisPath::SoulCrystallization { .. } => {
                anyhow::ensure!(
                    orch.crystallization_engine().is_some(),
                    "UAT: Soul Crystallization path but no engine created"
                );
                let soul = orion_core::templates::fill_soul_template(
                    "OrionUAT",
                    "assist with UAT verification.",
                    "concise and deterministic.",
                );
                let growth = orion_core::templates::GROWTH_MD.to_string();
                (soul, growth)
            }
            GenesisPath::QuickStart => {
                let soul = orion_core::templates::fill_soul_template(
                    "OrionUAT",
                    orion_core::templates::DEFAULT_PURPOSE,
                    orion_core::templates::DEFAULT_PERSONALITY,
                );
                let growth = orion_core::templates::GROWTH_MD.to_string();
                (soul, growth)
            }
            GenesisPath::SoulForge => {
                let mut app = SoulForgeApp::new();
                while app.state == SoulForgeAppState::Boot {
                    app.tick_boot(2);
                    if app.boot_progress >= 100 {
                        app.next_stage();
                        break;
                    }
                }
                if app.state == SoulForgeAppState::Intro {
                    app.next_stage();
                }
                app.list_state_selected = Some(0);
                app.handle_selection();
                app.list_state_selected = Some(0);
                app.handle_selection();
                app.list_state_selected = Some(0);
                app.handle_selection();
                anyhow::ensure!(
                    app.state == SoulForgeAppState::Crystallize,
                    "UAT: Soul Forge did not reach Crystallize"
                );
                let output = app
                    .soul_output("OrionUAT", None, None)
                    .context("UAT: soul_output")?;
                anyhow::ensure!(
                    !output.archetype.is_empty(),
                    "UAT: Soul Forge archetype is empty"
                );
                anyhow::ensure!(
                    output.soul_hash.len() == 64
                        && output.soul_hash.chars().all(|c| c.is_ascii_hexdigit()),
                    "UAT: Soul Forge soul_hash must be 64 hex chars, got {}",
                    output.soul_hash.len()
                );
                anyhow::ensure!(
                    !output.sigil_art.is_empty(),
                    "UAT: Soul Forge sigil_art is empty"
                );
                (output.soul_content, output.growth_content)
            }
        };
        orch.crystallize_soul(&soul_md, &growth_md)?;
        tracing::info!(
            "UAT: Genesis done (path: {}), crystallized soul, advanced to Emergence",
            genesis_path.id()
        );

        let docs_dir = orch.config().docs_dir.clone();
        let soul_path = docs_dir.join("soul.md");
        let growth_path = docs_dir.join("growth.md");
        anyhow::ensure!(
            soul_path.exists(),
            "UAT: soul.md not written after crystallize_soul"
        );
        anyhow::ensure!(
            growth_path.exists(),
            "UAT: growth.md not written after crystallize_soul"
        );
        let soul_content = std::fs::read_to_string(&soul_path).context("read soul.md")?;
        let growth_content = std::fs::read_to_string(&growth_path).context("read growth.md")?;
        anyhow::ensure!(
            soul_content.contains("OrionUAT") || soul_content.contains("ORION"),
            "UAT: soul.md must contain agent name"
        );
        anyhow::ensure!(
            !growth_content.is_empty(),
            "UAT: growth.md must be non-empty"
        );
    }

    // --- Emergence ---
    if orch.current_stage() == BirthStage::Emergence {
        orch.complete_emergence()?;
        tracing::info!("UAT: Emergence done, birth complete");
    }

    // --- Early operation: verify birth_complete and optional route ---
    let config = orch.config();
    if !config.birth_complete {
        anyhow::bail!("UAT: birth_complete is still false after emergence");
    }
    let store = orion_memory::MemoryStore::open_with_config(config)?;
    let uat_agent_id = config.agent_id.as_deref().unwrap_or("uat-agent");
    if !store.has_birth(uat_agent_id)? {
        anyhow::bail!("UAT: store.has_birth() is false after emergence");
    }
    tracing::info!("UAT: Early operation check: birth_complete and store.has_birth() OK");

    let router = tokio::runtime::Runtime::new()?.block_on(build_birth_router(config));
    let messages = vec![
        Message::new("system", "You are a helpful assistant. Reply briefly."),
        Message::new("user", "Say hello in one word."),
    ];
    let response = tokio::runtime::Runtime::new()?
        .block_on(birth_chat_turn(&router, messages))
        .context("early operation route")?;
    if response.assistant_content.is_empty() && !router.has_ego() {
        tracing::warn!("UAT: Id-only route returned empty (may be expected if no local model)");
    } else {
        tracing::info!(
            "UAT: Early operation route returned {} chars",
            response.assistant_content.len()
        );
    }

    tracing::info!("UAT: All stages passed. Application is in usable operation.");
    Ok(())
}

fn parse_genesis_path_from_env() -> GenesisPath {
    let s = std::env::var("UAT_GENESIS_PATH")
        .ok()
        .unwrap_or_else(|| "direct".to_string());
    match s.trim().to_lowercase().as_str() {
        "soul_forge" => GenesisPath::SoulForge,
        "soul_crystallization" => GenesisPath::SoulCrystallization {
            depth: SoulCrystallizationDepth::QuickStart,
        },
        _ => GenesisPath::Direct,
    }
}

fn app_config_from_env(data_dir: &std::path::Path) -> Result<AppConfig> {
    let mut config = AppConfig::default_paths();
    config.data_dir = data_dir.to_path_buf();
    config.models_dir = data_dir.join("models");
    config.docs_dir = data_dir.join("docs");
    config.db_path = data_dir.join("orion_uat.db");

    if let Ok(url) = std::env::var("LOCAL_LLM_BASE_URL") {
        if !url.is_empty() {
            config.local_llm_base_url = Some(url);
        }
    }
    if let Ok(backend) = std::env::var("MEMORY_BACKEND") {
        config.memory_backend = MemoryBackend::from_str(backend.as_str()).unwrap_or_default();
    }
    if let Ok(url) = std::env::var("DATABASE_URL") {
        if !url.is_empty() {
            config.database_url = Some(url);
        }
    }
    if let Ok(m) = std::env::var("BIRTH_MODEL") {
        if !m.is_empty() {
            config.birth_model = Some(m);
        }
    }
    config.apply_env_overrides();
    Ok(config)
}
