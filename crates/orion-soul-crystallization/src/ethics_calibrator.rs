//! Maps MentorProfile to Triangle Ethic weights.

use crate::models::{AttachmentStyle, MentorProfile, TriangleEthicWeights};

/// Calibrate Triangle Ethic weights from a MentorProfile.
pub fn calibrate_triangle_ethic(profile: &MentorProfile) -> TriangleEthicWeights {
    let mf = &profile.moral_foundations;
    let ocean = &profile.ocean;

    let mut deontological = (mf.authority + mf.sanctity) / 2.0;
    let mut areteological = (mf.care + mf.fairness) / 2.0;
    let mut teleological = (mf.liberty + mf.loyalty) / 2.0;

    let ocean_scale = 0.1;
    deontological += (ocean.conscientiousness - 0.5) * ocean_scale;
    areteological += (ocean.agreeableness - 0.5) * ocean_scale;
    teleological += (ocean.openness - 0.5) * ocean_scale;

    let attachment_scale = 0.03;
    match profile.attachment_style {
        AttachmentStyle::Secure => {
            areteological += attachment_scale;
        }
        AttachmentStyle::Anxious => {
            deontological += attachment_scale;
        }
        AttachmentStyle::Avoidant => {
            teleological += attachment_scale;
        }
        AttachmentStyle::Disorganized => {}
    }

    deontological = deontological.max(0.01);
    areteological = areteological.max(0.01);
    teleological = teleological.max(0.01);

    let mut weights = TriangleEthicWeights {
        deontological,
        areteological,
        teleological,
    };
    weights.normalize();
    weights
}
