//! SoulManifest: the standardized output contract for all Genesis paths.

use crate::ethics::CompassEthicWeights;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Standardized output produced by every Genesis path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulManifest {
    /// Agent name chosen during Genesis.
    pub agent_name: String,
    /// Mentor name (defaults to "Mentor" for Quick Start).
    pub mentor_name: String,
    /// Archetype determined by the path (e.g., "The Iron Sentinel").
    pub archetype: String,
    /// Complete soul.md content (Genesis Block appended by caller via `append_genesis_block`).
    pub soul_content: String,
    /// Complete growth.md content.
    pub growth_content: String,
    /// Normalized Compass Ethic weights.
    pub ethics_weights: CompassEthicWeights,
    /// Name of the birth method (e.g., "Soul Forge v2", "Direct Discovery").
    pub birth_method: String,
    /// SHA-256 provenance hash.
    pub genesis_hash: String,

    // Optional fields
    /// Extended archetype description.
    pub archetype_detail: Option<String>,
    /// ASCII/Unicode sigil art.
    pub sigil_art: Option<String>,
    /// Psychometric signals from crystallization.
    pub raw_signals: Option<Vec<serde_json::Value>>,
    /// Full soul data as JSON.
    pub soul_json: Option<serde_json::Value>,
}

impl SoulManifest {
    /// Compute a genesis hash from the manifest content.
    pub fn compute_genesis_hash(
        agent_name: &str,
        archetype: &str,
        weights: &CompassEthicWeights,
        birth_method: &str,
        entropy: &[String],
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(agent_name.as_bytes());
        hasher.update(archetype.as_bytes());
        hasher.update(
            format!(
                "{:.6}{:.6}{:.6}{:.6}",
                weights.duty, weights.virtue, weights.outcome, weights.welfare
            )
            .as_bytes(),
        );
        hasher.update(birth_method.as_bytes());
        for e in entropy {
            hasher.update(e.as_bytes());
        }
        let result = hasher.finalize();
        format!("{:x}", result)
    }

    /// Append a Genesis Block provenance section to soul_content.
    pub fn append_genesis_block(&mut self) {
        let timestamp = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let hash_display = if self.genesis_hash.len() > 16 {
            &self.genesis_hash[..16]
        } else {
            &self.genesis_hash
        };

        let block = format!(
            r#"

## Genesis Block

- **Mentor**: {}
- **Archetype**: {}
- **Birth Method**: {}
- **Compass Ethic**:
  - Duty (North): {:.2}
  - Virtue (East): {:.2}
  - Outcome (South): {:.2}
  - Welfare (West): {:.2}
- **Genesis Hash**: `{}...`
- **Timestamp**: {}
"#,
            self.mentor_name,
            self.archetype,
            self.birth_method,
            self.ethics_weights.duty,
            self.ethics_weights.virtue,
            self.ethics_weights.outcome,
            self.ethics_weights.welfare,
            hash_display,
            timestamp,
        );

        self.soul_content.push_str(&block);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manifest() -> SoulManifest {
        SoulManifest {
            agent_name: "Orion".to_string(),
            mentor_name: "Jason".to_string(),
            archetype: "The Iron Sentinel".to_string(),
            soul_content: "# Soul\n\nI am Orion.".to_string(),
            growth_content: "# Growth\n".to_string(),
            ethics_weights: CompassEthicWeights::new(0.35, 0.25, 0.20, 0.20),
            birth_method: "Soul Forge v2".to_string(),
            genesis_hash: "a3f8c9d2e1b7a4f6c0d9e8f7a6b5c4d3".to_string(),
            archetype_detail: None,
            sigil_art: None,
            raw_signals: None,
            soul_json: None,
        }
    }

    #[test]
    fn test_append_genesis_block() {
        let mut m = make_manifest();
        m.append_genesis_block();
        assert!(m.soul_content.contains("## Genesis Block"));
        assert!(m.soul_content.contains("Jason"));
        assert!(m.soul_content.contains("The Iron Sentinel"));
        assert!(m.soul_content.contains("Soul Forge v2"));
        assert!(m.soul_content.contains("Duty (North)"));
        assert!(m.soul_content.contains("Welfare (West)"));
    }

    #[test]
    fn test_compute_genesis_hash_deterministic() {
        let weights = CompassEthicWeights::new(0.4, 0.3, 0.2, 0.1);
        let h1 = SoulManifest::compute_genesis_hash(
            "Orion",
            "Sentinel",
            &weights,
            "forge",
            &["a".into()],
        );
        let h2 = SoulManifest::compute_genesis_hash(
            "Orion",
            "Sentinel",
            &weights,
            "forge",
            &["a".into()],
        );
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }
}
