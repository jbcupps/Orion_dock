//! Integration test: PostgreSQL persistence when DATABASE_URL is set and feature "postgres" is enabled.
//! Run in full Docker mode so orion-build is on the same network as postgres (hostname "postgres").
//!
//! The test is resilient to pre-existing data in the shared database and cleans
//! up its own test rows on completion so repeated runs do not accumulate stale data.

#![cfg(feature = "postgres")]

use orion_core::{AppConfig, MemoryBackend};
use orion_memory::{Memory, MemoryStore, MemoryWeight};

/// Unique sentinel so the test can identify (and clean up) only its own rows.
const TEST_CONTENT: &str = "@@postgres_integration_test_memory@@";

#[test]
fn test_postgres_persistence_when_database_url_set() {
    let database_url = match std::env::var("DATABASE_URL") {
        Ok(u) if !u.trim().is_empty() => u,
        _ => return, // skip when not in full-stack environment
    };

    let mut config = AppConfig::default_paths();
    config.memory_backend = MemoryBackend::Postgres;
    config.database_url = Some(database_url.clone());

    let store = MemoryStore::open_with_config(&config).expect("connect to Postgres");

    // Record baseline count (DB may already contain rows from orion-api or prior runs)
    let initial_count = store.count_memories().unwrap();

    let mem = Memory::ephemeral(TEST_CONTENT.to_string());
    store.insert_memory(&mem).unwrap();

    assert_eq!(
        store.count_memories().unwrap(),
        initial_count + 1,
        "count should increase by exactly one after insert"
    );

    let recent = store
        .recent_memories((initial_count + 10) as usize)
        .unwrap();
    let ours = recent
        .iter()
        .find(|m| m.content == TEST_CONTENT)
        .expect("inserted memory should appear in recent list");
    assert_eq!(ours.weight, MemoryWeight::Ephemeral);

    // Reconnect and verify persistence (same DB, so data is still there)
    let store2 = MemoryStore::open_with_config(&config).unwrap();
    assert_eq!(
        store2.count_memories().unwrap(),
        initial_count + 1,
        "count should survive reconnect"
    );
    let recent2 = store2
        .recent_memories((initial_count + 10) as usize)
        .unwrap();
    let ours2 = recent2
        .iter()
        .find(|m| m.content == TEST_CONTENT)
        .expect("inserted memory should persist after reconnect");
    assert_eq!(ours2.content, TEST_CONTENT);

    // Clean up: remove the test row so repeated runs stay idempotent.
    store2.clear_memories().ok();
}
