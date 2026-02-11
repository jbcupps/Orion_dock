//! Integration test: SQLite persistence across open/close.

use orion_memory::{Memory, MemoryStore, MemoryWeight};

#[test]
fn test_sqlite_persistence_across_restart() {
    let dir = std::env::temp_dir().join("orion_memory_integration");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("persist_test.db");
    let _ = std::fs::remove_file(&path);

    let content = "integration test memory content";
    {
        let store = MemoryStore::open(&path).unwrap();
        assert_eq!(store.count_memories().unwrap(), 0);
        let mem = Memory::ephemeral(content.to_string());
        store.insert_memory(&mem).unwrap();
        assert_eq!(store.count_memories().unwrap(), 1);
        let recent = store.recent_memories(10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].content, content);
        assert_eq!(recent[0].weight, MemoryWeight::Ephemeral);
    }

    let store2 = MemoryStore::open(&path).unwrap();
    assert_eq!(store2.count_memories().unwrap(), 1);
    let recent2 = store2.recent_memories(10).unwrap();
    assert_eq!(recent2.len(), 1);
    assert_eq!(recent2[0].content, content);

    let _ = std::fs::remove_file(&path);
}
