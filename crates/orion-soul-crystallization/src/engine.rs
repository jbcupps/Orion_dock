//! CrystallizationEngine: state machine driving the crystallization process.
//!
//! Supports orion-birth-style text-based tool_request blocks (no abigail-capabilities dependency).

use crate::ethics_calibrator::calibrate_triangle_ethic;
use crate::models::{ConversationTurn, CrystallizationPhase, DepthLevel, MentorProfile, Signal};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// A single tool request (orion-birth compatible: name + arguments string).
#[derive(Debug, Clone)]
pub struct ToolRequest {
    pub name: String,
    pub arguments: String,
}

/// Result of processing an LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessResult {
    pub display_text: String,
    pub phase_complete: bool,
    pub current_phase: CrystallizationPhase,
    pub signals_extracted: Vec<Signal>,
}

/// Status summary of the crystallization engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrystallizationStatus {
    pub phase: CrystallizationPhase,
    pub depth: DepthLevel,
    pub turn_count: usize,
    pub profile_completeness: f64,
}

/// The crystallization engine state machine.
pub struct CrystallizationEngine {
    profile: MentorProfile,
    depth: DepthLevel,
    current_phase: CrystallizationPhase,
    conversation_history: Vec<ConversationTurn>,
    turn_count: usize,
    used_question_ids: HashSet<String>,
}

impl CrystallizationEngine {
    pub fn new(depth: DepthLevel) -> Self {
        let initial_phase = match depth {
            DepthLevel::QuickStart => CrystallizationPhase::Crystallization,
            DepthLevel::Conversation | DepthLevel::DeepDive => CrystallizationPhase::Lattice,
        };

        Self {
            profile: MentorProfile::new(depth),
            depth,
            current_phase: initial_phase,
            conversation_history: Vec::new(),
            turn_count: 0,
            used_question_ids: HashSet::new(),
        }
    }

    pub fn current_phase(&self) -> CrystallizationPhase {
        self.current_phase
    }

    pub fn depth(&self) -> DepthLevel {
        self.depth
    }

    pub fn turn_count(&self) -> usize {
        self.turn_count
    }

    pub fn profile(&self) -> &MentorProfile {
        &self.profile
    }

    pub fn profile_mut(&mut self) -> &mut MentorProfile {
        &mut self.profile
    }

    pub fn conversation_history(&self) -> &[ConversationTurn] {
        &self.conversation_history
    }

    pub fn status(&self) -> CrystallizationStatus {
        CrystallizationStatus {
            phase: self.current_phase,
            depth: self.depth,
            turn_count: self.turn_count,
            profile_completeness: self.profile.completeness(),
        }
    }

    pub fn add_user_message(&mut self, content: &str) {
        self.conversation_history.push(ConversationTurn {
            role: "user".to_string(),
            content: content.to_string(),
            signals: Vec::new(),
        });
        self.turn_count += 1;
        self.profile.turn_count = self.turn_count;
    }

    /// Process an LLM response using orion-birth-style tool requests (name + arguments).
    pub fn process_response_from_tool_requests(
        &mut self,
        assistant_content: &str,
        tool_requests: &[ToolRequest],
    ) -> ProcessResult {
        let mut signals_extracted = Vec::new();

        for tr in tool_requests {
            if tr.name == "record_signal" {
                if let Ok(signals) = parse_record_signal_args(&tr.arguments) {
                    for signal in &signals {
                        self.profile.apply_signal(signal);
                    }
                    signals_extracted.extend(signals);
                }
            }
        }

        self.conversation_history.push(ConversationTurn {
            role: "assistant".to_string(),
            content: assistant_content.to_string(),
            signals: signals_extracted.clone(),
        });

        let phase_complete = self.should_advance_phase();

        ProcessResult {
            display_text: assistant_content.to_string(),
            phase_complete,
            current_phase: self.current_phase,
            signals_extracted,
        }
    }

    pub fn should_advance_phase(&self) -> bool {
        match self.current_phase {
            CrystallizationPhase::Awakening => false,
            CrystallizationPhase::Lattice => {
                let ocean_ok = self.profile.avg_ocean_confidence() >= 0.4;
                let moral_ok = self.profile.avg_moral_confidence() >= 0.3;
                let attachment_ok = self.profile.attachment_confidence >= 0.5;
                let hard_cap = self.turn_count >= 10;
                (ocean_ok && moral_ok && attachment_ok) || hard_cap
            }
            CrystallizationPhase::Reflection => self.profile.mirror_text.is_some(),
            CrystallizationPhase::Crucible => false,
            CrystallizationPhase::Crystallization => true,
            CrystallizationPhase::Complete => false,
        }
    }

    pub fn advance_phase(&mut self) -> Option<CrystallizationPhase> {
        let next = match (self.current_phase, self.depth) {
            (CrystallizationPhase::Awakening, _) => CrystallizationPhase::Lattice,
            (CrystallizationPhase::Lattice, _) => CrystallizationPhase::Reflection,
            (CrystallizationPhase::Reflection, DepthLevel::DeepDive) => {
                CrystallizationPhase::Crucible
            }
            (CrystallizationPhase::Reflection, _) => CrystallizationPhase::Crystallization,
            (CrystallizationPhase::Crucible, _) => CrystallizationPhase::Crystallization,
            (CrystallizationPhase::Crystallization, _) => CrystallizationPhase::Complete,
            (CrystallizationPhase::Complete, _) => return None,
        };

        self.current_phase = next;
        Some(next)
    }

    pub fn calibrate_ethics(&mut self) {
        self.profile.ethics_weights = calibrate_triangle_ethic(&self.profile);
        self.profile.compass_weights =
            crate::ethics_calibrator::calibrate_compass_ethic(&self.profile);
    }

    pub fn mark_question_used(&mut self, id: &str) {
        self.used_question_ids.insert(id.to_string());
    }

    pub fn is_question_used(&self, id: &str) -> bool {
        self.used_question_ids.contains(id)
    }

    pub fn set_mirror_text(&mut self, text: String) {
        self.profile.mirror_text = Some(text);
    }

    pub fn set_timestamp(&mut self, timestamp: String) {
        self.profile.timestamp = Some(timestamp);
    }
}

fn parse_record_signal_args(arguments: &str) -> Result<Vec<Signal>, serde_json::Error> {
    #[derive(Deserialize)]
    struct RecordSignalArgs {
        signals: Vec<Signal>,
    }

    if let Ok(args) = serde_json::from_str::<RecordSignalArgs>(arguments) {
        return Ok(args.signals);
    }
    if let Ok(signal) = serde_json::from_str::<Signal>(arguments) {
        return Ok(vec![signal]);
    }
    serde_json::from_str::<Vec<Signal>>(arguments)
}
