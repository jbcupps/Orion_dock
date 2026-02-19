//! Data models for the Soul Crystallization Protocol.
//!
//! Contains the MentorProfile, psychological instrument scores,
//! ethical framework weights, and conversation state types.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Depth level chosen by the mentor at the Spark phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DepthLevel {
    /// ~30 seconds: uses existing template system, no conversation.
    QuickStart,
    /// 3-5 minutes: adaptive Socratic dialogue (6-10 turns).
    Conversation,
    /// 10-15 minutes: full dialogue + ethical dilemmas + naming ceremony.
    DeepDive,
}

impl DepthLevel {
    pub fn label(&self) -> &'static str {
        match self {
            DepthLevel::QuickStart => "Quick Start",
            DepthLevel::Conversation => "Conversation",
            DepthLevel::DeepDive => "Deep Dive",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            DepthLevel::QuickStart => "quick_start",
            DepthLevel::Conversation => "conversation",
            DepthLevel::DeepDive => "deep_dive",
        }
    }

    pub fn estimated_time(&self) -> &'static str {
        match self {
            DepthLevel::QuickStart => "~30 seconds",
            DepthLevel::Conversation => "3-5 minutes",
            DepthLevel::DeepDive => "10-15 minutes",
        }
    }
}

/// Internal phases of the crystallization process.
/// Renamed from Spark/Conversation/Mirror/Forge/SoulGeneration to
/// Awakening/Lattice/Reflection/Crucible/Crystallization for richer semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrystallizationPhase {
    /// Phase 0: Intro + mentor name confirmation ("The Awakening").
    #[serde(alias = "Spark")]
    Awakening,
    /// Phase 1: Adaptive Socratic dialogue with structured probes ("The Lattice").
    #[serde(alias = "Conversation")]
    Lattice,
    /// Phase 2: Draft identity card + iterative correction ("The Reflection").
    #[serde(alias = "Mirror")]
    Reflection,
    /// Phase 3: Ethical dilemmas from shared TOML pool ("The Crucible", Deep Dive only).
    #[serde(alias = "Forge")]
    Crucible,
    /// Phase 4: Soul document generation and review ("The Crystallization").
    #[serde(alias = "SoulGeneration")]
    Crystallization,
    /// Terminal: crystallization complete.
    Complete,
}

/// Big Five (OCEAN) personality scores.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OceanScores {
    pub openness: f64,
    pub conscientiousness: f64,
    pub extraversion: f64,
    pub agreeableness: f64,
    pub neuroticism: f64,
}

impl OceanScores {
    pub fn neutral() -> Self {
        Self {
            openness: 0.5,
            conscientiousness: 0.5,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.5,
        }
    }
}

/// Moral Foundations Theory scores (Haidt).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MoralFoundations {
    pub care: f64,
    pub fairness: f64,
    pub loyalty: f64,
    pub authority: f64,
    pub sanctity: f64,
    pub liberty: f64,
}

impl MoralFoundations {
    pub fn neutral() -> Self {
        Self {
            care: 0.5,
            fairness: 0.5,
            loyalty: 0.5,
            authority: 0.5,
            sanctity: 0.5,
            liberty: 0.5,
        }
    }
}

/// Attachment style of the mentor (simplified).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AttachmentStyle {
    #[default]
    Secure,
    Anxious,
    Avoidant,
    Disorganized,
}

/// Dominant thinking mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ThinkingMode {
    Analytical,
    Intuitive,
    #[default]
    Integrated,
}

/// Cognitive style preferences.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CognitiveStyle {
    pub thinking_mode: ThinkingMode,
    pub precision_vs_speed: f64,
    pub breadth_vs_depth: f64,
}

impl CognitiveStyle {
    pub fn neutral() -> Self {
        Self {
            thinking_mode: ThinkingMode::Integrated,
            precision_vs_speed: 0.5,
            breadth_vs_depth: 0.5,
        }
    }
}

/// Communication preferences (Phase 3 / Deep Dive).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunicationPreference {
    pub dimension: String,
    pub value: String,
}

/// Triangle Ethic weights (must sum to 1.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriangleEthicWeights {
    pub deontological: f64,
    pub areteological: f64,
    pub teleological: f64,
}

impl Default for TriangleEthicWeights {
    fn default() -> Self {
        Self {
            deontological: 1.0 / 3.0,
            areteological: 1.0 / 3.0,
            teleological: 1.0 / 3.0,
        }
    }
}

impl TriangleEthicWeights {
    pub fn normalize(&mut self) {
        let sum = self.deontological + self.areteological + self.teleological;
        if sum > 0.0 {
            self.deontological /= sum;
            self.areteological /= sum;
            self.teleological /= sum;
        } else {
            *self = Self::default();
        }
    }
}

/// A signal extracted from conversation by the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub instrument: String,
    pub dimension: String,
    pub value: f64,
    pub confidence: f64,
    pub reasoning: String,
}

/// A turn in the crystallization conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationTurn {
    pub role: String,
    pub content: String,
    pub signals: Vec<Signal>,
}

/// The accumulated MentorProfile built during crystallization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentorProfile {
    pub ocean: OceanScores,
    pub ocean_confidence: HashMap<String, f64>,
    pub moral_foundations: MoralFoundations,
    pub moral_foundations_confidence: HashMap<String, f64>,
    pub attachment_style: AttachmentStyle,
    pub attachment_confidence: f64,
    pub cognitive_style: CognitiveStyle,
    pub communication_preferences: Vec<CommunicationPreference>,
    /// Legacy 3-dimension Triangle Ethic weights (kept for backward compat).
    pub ethics_weights: TriangleEthicWeights,
    /// 4-dimension Compass Ethic weights (primary).
    #[serde(default)]
    pub compass_weights: genesis_core::CompassEthicWeights,
    pub mirror_text: Option<String>,
    pub name_choice: Option<String>,
    pub raw_signals: Vec<Signal>,
    pub depth: DepthLevel,
    pub turn_count: usize,
    pub timestamp: Option<String>,
}

impl MentorProfile {
    pub fn new(depth: DepthLevel) -> Self {
        Self {
            ocean: OceanScores::neutral(),
            ocean_confidence: HashMap::new(),
            moral_foundations: MoralFoundations::neutral(),
            moral_foundations_confidence: HashMap::new(),
            attachment_style: AttachmentStyle::default(),
            attachment_confidence: 0.0,
            cognitive_style: CognitiveStyle::neutral(),
            communication_preferences: Vec::new(),
            ethics_weights: TriangleEthicWeights::default(),
            compass_weights: genesis_core::CompassEthicWeights::default(),
            mirror_text: None,
            name_choice: None,
            raw_signals: Vec::new(),
            depth,
            turn_count: 0,
            timestamp: None,
        }
    }

    pub fn apply_signal(&mut self, signal: &Signal) {
        self.raw_signals.push(signal.clone());

        match signal.instrument.as_str() {
            "big_five" => self.apply_ocean_signal(signal),
            "moral_foundations" => self.apply_moral_signal(signal),
            "attachment" => self.apply_attachment_signal(signal),
            "cognitive" => self.apply_cognitive_signal(signal),
            _ => {
                tracing::debug!("Unknown instrument: {}", signal.instrument);
            }
        }
    }

    fn apply_ocean_signal(&mut self, signal: &Signal) {
        let current_confidence = self
            .ocean_confidence
            .get(&signal.dimension)
            .copied()
            .unwrap_or(0.0);

        let total_weight = current_confidence + signal.confidence;
        if total_weight > 0.0 {
            let blend = |old: f64, new: f64| -> f64 {
                (old * current_confidence + new * signal.confidence) / total_weight
            };

            match signal.dimension.as_str() {
                "openness" => self.ocean.openness = blend(self.ocean.openness, signal.value),
                "conscientiousness" => {
                    self.ocean.conscientiousness = blend(self.ocean.conscientiousness, signal.value)
                }
                "extraversion" => {
                    self.ocean.extraversion = blend(self.ocean.extraversion, signal.value)
                }
                "agreeableness" => {
                    self.ocean.agreeableness = blend(self.ocean.agreeableness, signal.value)
                }
                "neuroticism" => {
                    self.ocean.neuroticism = blend(self.ocean.neuroticism, signal.value)
                }
                _ => {}
            }

            self.ocean_confidence
                .insert(signal.dimension.clone(), total_weight.min(1.0));
        }
    }

    fn apply_moral_signal(&mut self, signal: &Signal) {
        let current_confidence = self
            .moral_foundations_confidence
            .get(&signal.dimension)
            .copied()
            .unwrap_or(0.0);

        let total_weight = current_confidence + signal.confidence;
        if total_weight > 0.0 {
            let blend = |old: f64, new: f64| -> f64 {
                (old * current_confidence + new * signal.confidence) / total_weight
            };

            match signal.dimension.as_str() {
                "care" => {
                    self.moral_foundations.care = blend(self.moral_foundations.care, signal.value)
                }
                "fairness" => {
                    self.moral_foundations.fairness =
                        blend(self.moral_foundations.fairness, signal.value)
                }
                "loyalty" => {
                    self.moral_foundations.loyalty =
                        blend(self.moral_foundations.loyalty, signal.value)
                }
                "authority" => {
                    self.moral_foundations.authority =
                        blend(self.moral_foundations.authority, signal.value)
                }
                "sanctity" => {
                    self.moral_foundations.sanctity =
                        blend(self.moral_foundations.sanctity, signal.value)
                }
                "liberty" => {
                    self.moral_foundations.liberty =
                        blend(self.moral_foundations.liberty, signal.value)
                }
                _ => {}
            }

            self.moral_foundations_confidence
                .insert(signal.dimension.clone(), total_weight.min(1.0));
        }
    }

    fn apply_attachment_signal(&mut self, signal: &Signal) {
        if signal.confidence > self.attachment_confidence {
            self.attachment_style = match signal.dimension.as_str() {
                "secure" => AttachmentStyle::Secure,
                "anxious" => AttachmentStyle::Anxious,
                "avoidant" => AttachmentStyle::Avoidant,
                "disorganized" => AttachmentStyle::Disorganized,
                _ => return,
            };
            self.attachment_confidence = signal.confidence;
        }
    }

    fn apply_cognitive_signal(&mut self, signal: &Signal) {
        match signal.dimension.as_str() {
            "thinking_mode" => {
                self.cognitive_style.thinking_mode = if signal.value < 0.33 {
                    ThinkingMode::Analytical
                } else if signal.value > 0.66 {
                    ThinkingMode::Intuitive
                } else {
                    ThinkingMode::Integrated
                };
            }
            "precision_vs_speed" => {
                self.cognitive_style.precision_vs_speed = signal.value;
            }
            "breadth_vs_depth" => {
                self.cognitive_style.breadth_vs_depth = signal.value;
            }
            _ => {}
        }
    }

    pub fn avg_ocean_confidence(&self) -> f64 {
        if self.ocean_confidence.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.ocean_confidence.values().sum();
        sum / self.ocean_confidence.len() as f64
    }

    pub fn avg_moral_confidence(&self) -> f64 {
        if self.moral_foundations_confidence.is_empty() {
            return 0.0;
        }
        let sum: f64 = self.moral_foundations_confidence.values().sum();
        sum / self.moral_foundations_confidence.len() as f64
    }

    pub fn completeness(&self) -> f64 {
        let ocean_score = self.avg_ocean_confidence();
        let moral_score = self.avg_moral_confidence();
        let attachment_score = self.attachment_confidence;
        ocean_score * 0.4 + moral_score * 0.3 + attachment_score * 0.3
    }
}
