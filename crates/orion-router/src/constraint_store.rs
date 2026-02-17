//! Persistent constraint store for the Execution Governor.
//!
//! Constraints discovered during execution are persisted to the agent's data directory
//! so that future Planner calls can avoid re-discovering them.

use std::path::{Path, PathBuf};

use orion_skills::structured_failure::Constraint;
use tracing::{info, warn};

/// Persistent store for constraints discovered during governed execution.
/// Stored as a JSON array in `{agent_data_dir}/governor_constraints.json`.
pub struct ConstraintStore {
    path: PathBuf,
    constraints: Vec<Constraint>,
}

impl ConstraintStore {
    /// Load constraints from disk, or create an empty store.
    pub fn load(agent_data_dir: &Path) -> Self {
        let path = agent_data_dir.join("governor_constraints.json");
        let constraints = if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                    warn!("Failed to parse constraint store: {}", e);
                    Vec::new()
                }),
                Err(e) => {
                    warn!("Failed to read constraint store: {}", e);
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        info!(
            "Loaded {} persistent constraints from {}",
            constraints.len(),
            path.display()
        );

        Self { path, constraints }
    }

    /// Save constraints to disk.
    pub fn save(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.constraints)
            .map_err(|e| format!("Serialize constraints: {}", e))?;
        std::fs::write(&self.path, json)
            .map_err(|e| format!("Write constraints to {}: {}", self.path.display(), e))?;
        Ok(())
    }

    /// Add new constraints (deduplicating by Display representation).
    pub fn add(&mut self, new_constraints: &[Constraint]) {
        let existing: std::collections::HashSet<String> =
            self.constraints.iter().map(|c| c.to_string()).collect();

        for c in new_constraints {
            if !existing.contains(&c.to_string()) {
                self.constraints.push(c.clone());
            }
        }
    }

    /// Get all constraints as human-readable strings for the Planner prompt.
    pub fn as_planner_constraints(&self) -> Vec<String> {
        self.constraints.iter().map(|c| c.to_string()).collect()
    }

    /// Get the raw constraint list.
    pub fn constraints(&self) -> &[Constraint] {
        &self.constraints
    }

    /// Number of stored constraints.
    pub fn len(&self) -> usize {
        self.constraints.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn load_empty() {
        let tmp = std::env::temp_dir().join("orion_constraint_test_empty");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let store = ConstraintStore::load(&tmp);
        assert!(store.is_empty());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn save_and_reload() {
        let tmp = std::env::temp_dir().join("orion_constraint_test_roundtrip");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut store = ConstraintStore::load(&tmp);
        store.add(&[
            Constraint::PathBlocked {
                blocked: "/app/vault".to_string(),
                use_instead: vec!["/app/agent-data".to_string()],
            },
            Constraint::HostUnreachable {
                host: "localhost:1143".to_string(),
                try_instead: Some("host.docker.internal:1143".to_string()),
            },
        ]);
        store.save().unwrap();

        let reloaded = ConstraintStore::load(&tmp);
        assert_eq!(reloaded.len(), 2);
        let strings = reloaded.as_planner_constraints();
        assert!(strings[0].contains("PATH BLOCKED"));
        assert!(strings[1].contains("HOST UNREACHABLE"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn deduplicates() {
        let tmp = std::env::temp_dir().join("orion_constraint_test_dedup");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();

        let mut store = ConstraintStore::load(&tmp);
        let constraint = Constraint::PathBlocked {
            blocked: "/app/vault".to_string(),
            use_instead: vec!["/app/agent-data".to_string()],
        };
        store.add(std::slice::from_ref(&constraint));
        store.add(&[constraint]);
        assert_eq!(store.len(), 1);

        let _ = fs::remove_dir_all(&tmp);
    }
}
