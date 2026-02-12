use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum DocumentTier {
    /// Signed at install time, private key discarded. Immutable forever.
    Constitutional = 0,
    /// Signed by mentor key. Mentor can modify.
    MentorEditable = 1,
}

impl DocumentTier {
    /// Stable, lowercase string representation for use in signable bytes.
    /// This value is part of the signature format and MUST NOT change.
    pub fn as_str(&self) -> &'static str {
        match self {
            DocumentTier::Constitutional => "constitutional",
            DocumentTier::MentorEditable => "mentor_editable",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreDocument {
    pub name: String,
    pub tier: DocumentTier,
    pub content: String,
    /// Base64-encoded ed25519 signature
    pub signature: String,
    pub signed_at: chrono::DateTime<chrono::Utc>,
}

impl CoreDocument {
    pub fn new(name: String, tier: DocumentTier, content: String) -> Self {
        Self {
            name,
            tier,
            content: content.clone(),
            signature: String::new(),
            signed_at: chrono::Utc::now(),
        }
    }

    /// Returns the bytes that should be signed: `"{name}|{tier}|{content}"` (UTF-8).
    /// Tier uses the stable `as_str()` value (e.g. `"constitutional"`).
    pub fn signable_bytes(&self) -> Vec<u8> {
        format!("{}|{}|{}", self.name, self.tier.as_str(), self.content).into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_as_str_stable() {
        assert_eq!(DocumentTier::Constitutional.as_str(), "constitutional");
        assert_eq!(DocumentTier::MentorEditable.as_str(), "mentor_editable");
    }

    #[test]
    fn test_signable_bytes_format() {
        let doc = CoreDocument::new(
            "soul.md".into(),
            DocumentTier::Constitutional,
            "I am Orion.".into(),
        );
        let bytes = doc.signable_bytes();
        let s = String::from_utf8(bytes).unwrap();
        assert_eq!(s, "soul.md|constitutional|I am Orion.");
    }
}
